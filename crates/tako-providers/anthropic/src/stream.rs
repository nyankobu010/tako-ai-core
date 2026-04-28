//! SSE → ChatChunk adapter for Anthropic's Messages streaming format.
//!
//! Anthropic streams a sequence of typed events:
//!
//! - `message_start` — initial usage snapshot
//! - `content_block_start` — opens a text or tool_use block
//! - `content_block_delta` — text or partial JSON for tool_use input
//! - `content_block_stop` — closes a block
//! - `message_delta` — incremental usage and stop_reason
//! - `message_stop` — final
//! - `ping` — keepalive (ignored)

use eventsource_stream::Eventsource;
use futures::stream::{BoxStream, StreamExt};
use serde::Deserialize;
use tako_core::{ChatChunk, FinishReason, TakoError, ToolCallDelta, Usage};

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum AnEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnMessageStart },
    #[serde(rename = "content_block_start")]
    ContentBlockStart { index: u32, content_block: AnBlockStart },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: AnBlockDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {
        #[allow(dead_code)]
        index: u32,
    },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: AnMessageDelta, #[serde(default)] usage: AnUsage },
    #[serde(rename = "message_stop")]
    MessageStop {},
    #[serde(rename = "ping")]
    Ping {},
    #[serde(rename = "error")]
    Error { error: AnErrorBody },
}

#[derive(Deserialize, Debug)]
struct AnMessageStart {
    #[serde(default)]
    usage: AnUsage,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnBlockStart {
    Text {
        #[serde(default)]
        #[allow(dead_code)]
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
    },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnBlockDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize, Debug)]
struct AnMessageDelta {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct AnUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Deserialize, Debug)]
struct AnErrorBody {
    #[serde(default)]
    message: String,
}

pub fn into_chat_stream(resp: reqwest::Response) -> BoxStream<'static, Result<ChatChunk, TakoError>> {
    let bytes = resp.bytes_stream();
    let events = bytes
        .map(|res| res.map_err(|e| std::io::Error::other(e.to_string())))
        .eventsource();

    let mut last_finish: Option<FinishReason> = None;
    let mut last_usage = Usage::default();
    // Index → (id, name) for active tool_use blocks; we emit name on each
    // delta so the consumer doesn't need to track this itself.
    let mut tool_blocks: std::collections::HashMap<u32, (String, String)> = std::collections::HashMap::new();

    let stream = async_stream::stream! {
        let mut events = Box::pin(events);
        while let Some(item) = events.next().await {
            let ev = match item {
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("{e}") });
                    last_finish = Some(FinishReason::Error);
                    break;
                }
                Ok(ev) => ev,
            };
            let parsed: AnEvent = match serde_json::from_str(&ev.data) {
                Ok(p) => p,
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("invalid frame: {e}") });
                    continue;
                }
            };
            match parsed {
                AnEvent::Ping { .. } => {}
                AnEvent::MessageStart { message } => {
                    last_usage.input_tokens = message.usage.input_tokens;
                    last_usage.output_tokens = message.usage.output_tokens;
                }
                AnEvent::ContentBlockStart { index, content_block } => {
                    if let AnBlockStart::ToolUse { id, name } = content_block {
                        tool_blocks.insert(index, (id.clone(), name.clone()));
                        yield Ok(ChatChunk::Delta {
                            text: None,
                            tool_calls: vec![ToolCallDelta {
                                index,
                                id: Some(id),
                                name: Some(name),
                                arguments_fragment: None,
                            }],
                        });
                    }
                }
                AnEvent::ContentBlockDelta { index, delta } => match delta {
                    AnBlockDelta::TextDelta { text } => {
                        yield Ok(ChatChunk::Delta { text: Some(text), tool_calls: vec![] });
                    }
                    AnBlockDelta::InputJsonDelta { partial_json } => {
                        yield Ok(ChatChunk::Delta {
                            text: None,
                            tool_calls: vec![ToolCallDelta {
                                index,
                                id: None,
                                name: None,
                                arguments_fragment: Some(partial_json),
                            }],
                        });
                    }
                },
                AnEvent::ContentBlockStop { .. } => {}
                AnEvent::MessageDelta { delta, usage } => {
                    if usage.output_tokens > 0 {
                        last_usage.output_tokens = usage.output_tokens;
                    }
                    if let Some(reason) = delta.stop_reason {
                        last_finish = Some(match reason.as_str() {
                            "end_turn" | "stop_sequence" => FinishReason::Stop,
                            "max_tokens" => FinishReason::Length,
                            "tool_use" => FinishReason::ToolCalls,
                            _ => FinishReason::Other,
                        });
                    }
                }
                AnEvent::MessageStop { .. } => {
                    break;
                }
                AnEvent::Error { error } => {
                    yield Ok(ChatChunk::Error { message: error.message });
                    last_finish = Some(FinishReason::Error);
                    break;
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
