//! SSE → ChatChunk adapter for Vertex AI Gemini streaming responses.
//!
//! Vertex's `:streamGenerateContent?alt=sse` returns each chunk as a partial
//! [`crate::convert::VxResponse`] in `data: {json}\n\n` lines. There is no
//! explicit `[DONE]` terminator — the stream ends when the body closes — but
//! candidates carry a `finishReason` on the final chunk.

use eventsource_stream::Eventsource;
use futures::stream::{BoxStream, StreamExt};
use tako_core::{ChatChunk, FinishReason, TakoError, ToolCallDelta, Usage};

use crate::convert::{VxResponse, finish_reason_from_vx};

pub fn into_chat_stream(
    resp: reqwest::Response,
) -> BoxStream<'static, Result<ChatChunk, TakoError>> {
    let bytes = resp.bytes_stream();
    let events = bytes
        .map(|res| res.map_err(|e| std::io::Error::other(e.to_string())))
        .eventsource();

    let stream = async_stream::stream! {
        let mut events = Box::pin(events);
        let mut last_finish: Option<FinishReason> = None;
        let mut last_usage = Usage::default();
        let mut tool_call_index: u32 = 0;

        while let Some(item) = events.next().await {
            match item {
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("{e}") });
                    last_finish = Some(FinishReason::Error);
                    break;
                }
                Ok(ev) => {
                    if ev.data.is_empty() || ev.data == "[DONE]" {
                        continue;
                    }
                    let frame: VxResponse = match serde_json::from_str(&ev.data) {
                        Ok(f) => f,
                        Err(e) => {
                            yield Ok(ChatChunk::Error { message: format!("invalid frame: {e}") });
                            continue;
                        }
                    };

                    if let Some(u) = frame.usage_metadata {
                        last_usage = Usage {
                            input_tokens: u.prompt_tokens,
                            output_tokens: u.output_tokens,
                        };
                    }

                    for cand in frame.candidates {
                        if let Some(reason) = cand.finish_reason.as_deref() {
                            last_finish = Some(finish_reason_from_vx(Some(reason)));
                        }

                        let mut text_buf = String::new();
                        let mut tool_calls: Vec<ToolCallDelta> = Vec::new();
                        if let Some(c) = cand.content {
                            for part in c.parts {
                                if let Some(t) = part.text {
                                    if !t.is_empty() {
                                        text_buf.push_str(&t);
                                    }
                                }
                                if let Some(fc) = part.function_call {
                                    tool_calls.push(ToolCallDelta {
                                        index: tool_call_index,
                                        id: Some(format!("vertex_call_{tool_call_index}")),
                                        name: Some(fc.name),
                                        arguments_fragment: Some(fc.args.to_string()),
                                    });
                                    tool_call_index += 1;
                                }
                            }
                        }

                        let text = (!text_buf.is_empty()).then_some(text_buf);
                        if text.is_some() || !tool_calls.is_empty() {
                            yield Ok(ChatChunk::Delta { text, tool_calls });
                        }
                    }
                }
            }
        }

        let mut final_finish = last_finish.unwrap_or(FinishReason::Other);
        // If the model emitted any tool calls and the upstream finish was
        // generic STOP, surface that to the orchestrator as ToolCalls.
        if tool_call_index > 0 && matches!(final_finish, FinishReason::Stop | FinishReason::Other) {
            final_finish = FinishReason::ToolCalls;
        }

        yield Ok(ChatChunk::End {
            finish_reason: final_finish,
            usage: last_usage,
        });
    };

    stream.boxed()
}
