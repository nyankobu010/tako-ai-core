//! NDJSON → ChatChunk adapter for Ollama's `/api/chat` streaming format.
//!
//! Each line of the response body is a complete JSON object. Frames
//! with `done: false` carry incremental tokens in `message.content`
//! (treated as text deltas). The terminating frame has `done: true`
//! and may carry tool-call structure plus token counts.

use futures::stream::{BoxStream, StreamExt};
use tako_core::{ChatChunk, ContentPart, FinishReason, TakoError, ToolCallDelta, Usage};
use tokio::io::AsyncBufReadExt;
use tokio_util::io::StreamReader;

use crate::convert::{OlResponse, from_ollama_response};

pub fn into_chat_stream(
    resp: reqwest::Response,
) -> BoxStream<'static, Result<ChatChunk, TakoError>> {
    let bytes = resp
        .bytes_stream()
        .map(|res| res.map_err(|e| std::io::Error::other(e.to_string())));
    let reader = StreamReader::new(bytes);
    let lines = tokio::io::BufReader::new(reader).lines();

    let stream = async_stream::stream! {
        let mut lines = lines;
        let mut last_finish: Option<FinishReason> = None;
        let mut last_usage = Usage::default();
        loop {
            match lines.next_line().await {
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("{e}") });
                    last_finish = Some(FinishReason::Error);
                    break;
                }
                Ok(None) => break,
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let frame: OlResponse = match serde_json::from_str(line) {
                        Ok(f) => f,
                        Err(e) => {
                            yield Ok(ChatChunk::Error { message: format!("invalid frame: {e}") });
                            continue;
                        }
                    };

                    if frame.done {
                        // Terminating frame: token counts arrive here, and
                        // tool-call structure (if any) is finalised. We feed
                        // the frame through the response converter to assemble
                        // ToolCallDelta values for any tool calls.
                        last_usage = Usage {
                            input_tokens: frame.prompt_eval_count,
                            output_tokens: frame.eval_count,
                        };
                        let last_text = frame.message.content.clone();
                        let parsed_for_tools = OlResponse {
                            message: frame.message,
                            done: true,
                            done_reason: frame.done_reason.clone(),
                            prompt_eval_count: frame.prompt_eval_count,
                            eval_count: frame.eval_count,
                        };
                        if let Ok(resp) = from_ollama_response(parsed_for_tools) {
                            // The terminating frame may carry one final
                            // chunk of text alongside any tool calls; surface
                            // both. Emit text first so cumulative text matches
                            // the non-streaming path.
                            let mut deltas: Vec<ToolCallDelta> = Vec::new();
                            for (i, c) in resp.message.content.iter().enumerate() {
                                if let ContentPart::ToolCall { id, name, args } = c {
                                    deltas.push(ToolCallDelta {
                                        index: i as u32,
                                        id: Some(id.clone()),
                                        name: Some(name.clone()),
                                        arguments_fragment: Some(args.to_string()),
                                    });
                                }
                            }
                            let text = if last_text.is_empty() {
                                None
                            } else {
                                Some(last_text)
                            };
                            if text.is_some() || !deltas.is_empty() {
                                yield Ok(ChatChunk::Delta {
                                    text,
                                    tool_calls: deltas,
                                });
                            }
                            last_finish = Some(resp.finish_reason);
                        }
                        break;
                    }

                    let text = if frame.message.content.is_empty() {
                        None
                    } else {
                        Some(frame.message.content)
                    };
                    if text.is_some() {
                        yield Ok(ChatChunk::Delta {
                            text,
                            tool_calls: vec![],
                        });
                    }
                }
            }
        }
        yield Ok(ChatChunk::End {
            finish_reason: last_finish.unwrap_or(FinishReason::Stop),
            usage: last_usage,
        });
    };

    stream.boxed()
}
