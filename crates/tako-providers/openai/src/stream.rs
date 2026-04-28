//! SSE → ChatChunk adapter for OpenAI's chat.completions streaming format.

use eventsource_stream::Eventsource;
use futures::stream::{BoxStream, StreamExt};
use serde::Deserialize;
use tako_core::{ChatChunk, FinishReason, TakoError, ToolCallDelta, Usage};

#[derive(Deserialize, Debug)]
struct OaStreamFrame {
    #[serde(default)]
    choices: Vec<OaStreamChoice>,
    #[serde(default)]
    usage: Option<OaStreamUsage>,
}

#[derive(Deserialize, Debug)]
struct OaStreamChoice {
    #[serde(default)]
    delta: OaStreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct OaStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OaStreamToolCallDelta>,
}

#[derive(Deserialize, Debug)]
struct OaStreamToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OaStreamFnDelta>,
}

#[derive(Deserialize, Debug)]
struct OaStreamFnDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize, Debug)]
struct OaStreamUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

/// Wrap a `reqwest::Response` body in an SSE parser and emit `ChatChunk`s.
/// Always terminates with exactly one `ChatChunk::End`.
pub fn into_chat_stream(resp: reqwest::Response) -> BoxStream<'static, Result<ChatChunk, TakoError>> {
    let bytes = resp.bytes_stream();
    let events = bytes
        .map(|res| res.map_err(|e| std::io::Error::other(e.to_string())))
        .eventsource();

    let mut last_finish: Option<FinishReason> = None;
    let mut last_usage = Usage::default();

    let stream = async_stream::stream! {
        let mut events = Box::pin(events);
        while let Some(item) = events.next().await {
            match item {
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("{e}") });
                    last_finish = Some(FinishReason::Error);
                    break;
                }
                Ok(ev) => {
                    if ev.data == "[DONE]" {
                        break;
                    }
                    let frame: OaStreamFrame = match serde_json::from_str(&ev.data) {
                        Ok(f) => f,
                        Err(e) => {
                            yield Ok(ChatChunk::Error { message: format!("invalid frame: {e}") });
                            continue;
                        }
                    };

                    if let Some(u) = frame.usage {
                        last_usage = Usage {
                            input_tokens: u.prompt_tokens,
                            output_tokens: u.completion_tokens,
                        };
                    }

                    for choice in frame.choices {
                        if let Some(reason) = choice.finish_reason {
                            last_finish = Some(match reason.as_str() {
                                "stop" => FinishReason::Stop,
                                "length" => FinishReason::Length,
                                "tool_calls" => FinishReason::ToolCalls,
                                "content_filter" => FinishReason::ContentFilter,
                                _ => FinishReason::Other,
                            });
                        }

                        let text = choice.delta.content.filter(|s| !s.is_empty());
                        let tool_calls: Vec<ToolCallDelta> = choice
                            .delta
                            .tool_calls
                            .into_iter()
                            .map(|tc| ToolCallDelta {
                                index: tc.index,
                                id: tc.id,
                                name: tc.function.as_ref().and_then(|f| f.name.clone()),
                                arguments_fragment: tc.function.and_then(|f| f.arguments),
                            })
                            .collect();

                        if text.is_some() || !tool_calls.is_empty() {
                            yield Ok(ChatChunk::Delta { text, tool_calls });
                        }
                    }
                }
            }
        }
        yield Ok(ChatChunk::End {
            finish_reason: last_finish.unwrap_or(FinishReason::Other),
            usage: last_usage,
        });
    };

    stream.boxed()
}
