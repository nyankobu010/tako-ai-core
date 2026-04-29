//! Bedrock ConverseStream → ChatChunk adapter.
//!
//! `aws-sdk-bedrockruntime`'s `converse_stream()` returns a smithy
//! [`EventReceiver`](aws_sdk_bedrockruntime::event_receiver::EventReceiver)
//! emitting [`ConverseStreamOutput`] events (MessageStart, ContentBlockStart,
//! ContentBlockDelta, ContentBlockStop, MessageStop, Metadata, plus a
//! transport-level error). We map each event onto the tako [`ChatChunk`]
//! enum and always finish with a single `ChatChunk::End` (Phase-1 streaming
//! contract).

use aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamOutput;
use aws_sdk_bedrockruntime::types as br;
use aws_sdk_bedrockruntime::types::ConverseStreamOutput as Event;
use futures::stream::{BoxStream, StreamExt};
use tako_core::{ChatChunk, FinishReason, TakoError, ToolCallDelta, Usage};

/// Map a single Bedrock stream event to zero or more `ChatChunk`s, plus
/// optional finish-reason / usage updates that the caller accumulates and
/// emits in the terminal `ChatChunk::End`.
///
/// Returned tuple: `(chunks_to_yield, maybe_finish, maybe_usage)`.
pub(crate) fn map_event(
    event: &Event,
    tool_call_state: &mut std::collections::HashMap<i32, u32>,
) -> (Vec<ChatChunk>, Option<FinishReason>, Option<Usage>) {
    match event {
        Event::MessageStart(_) => (Vec::new(), None, None),

        Event::ContentBlockStart(start) => {
            let block_index = start.content_block_index();
            // Allocate a stable ToolCallDelta.index for this block_index so
            // downstream consumers can correlate later ToolUse deltas.
            let tc_index = u32::try_from(tool_call_state.len()).unwrap_or(u32::MAX);
            tool_call_state.insert(block_index, tc_index);

            let Some(start_inner) = start.start.as_ref() else {
                return (Vec::new(), None, None);
            };
            if let br::ContentBlockStart::ToolUse(tu) = start_inner {
                let chunk = ChatChunk::Delta {
                    text: None,
                    tool_calls: vec![ToolCallDelta {
                        index: tc_index,
                        id: Some(tu.tool_use_id.clone()),
                        name: Some(tu.name.clone()),
                        arguments_fragment: None,
                    }],
                };
                return (vec![chunk], None, None);
            }
            (Vec::new(), None, None)
        }

        Event::ContentBlockDelta(delta_event) => {
            let block_index = delta_event.content_block_index();
            let Some(delta) = delta_event.delta.as_ref() else {
                return (Vec::new(), None, None);
            };
            match delta {
                br::ContentBlockDelta::Text(t) => {
                    if t.is_empty() {
                        return (Vec::new(), None, None);
                    }
                    (
                        vec![ChatChunk::Delta {
                            text: Some(t.clone()),
                            tool_calls: Vec::new(),
                        }],
                        None,
                        None,
                    )
                }
                br::ContentBlockDelta::ToolUse(tu) => {
                    let next_index = u32::try_from(tool_call_state.len()).unwrap_or(u32::MAX);
                    let tc_index = *tool_call_state.entry(block_index).or_insert(next_index);
                    let chunk = ChatChunk::Delta {
                        text: None,
                        tool_calls: vec![ToolCallDelta {
                            index: tc_index,
                            id: None,
                            name: None,
                            arguments_fragment: Some(tu.input.clone()),
                        }],
                    };
                    (vec![chunk], None, None)
                }
                _ => (Vec::new(), None, None),
            }
        }

        Event::ContentBlockStop(_) => (Vec::new(), None, None),

        Event::MessageStop(stop) => {
            let reason = match stop.stop_reason() {
                br::StopReason::EndTurn | br::StopReason::StopSequence => FinishReason::Stop,
                br::StopReason::MaxTokens | br::StopReason::ModelContextWindowExceeded => {
                    FinishReason::Length
                }
                br::StopReason::ToolUse => FinishReason::ToolCalls,
                br::StopReason::ContentFiltered | br::StopReason::GuardrailIntervened => {
                    FinishReason::ContentFilter
                }
                br::StopReason::MalformedModelOutput | br::StopReason::MalformedToolUse => {
                    FinishReason::Error
                }
                _ => FinishReason::Other,
            };
            (Vec::new(), Some(reason), None)
        }

        Event::Metadata(meta) => {
            let usage = meta.usage().map(|u| Usage {
                input_tokens: u32::try_from(u.input_tokens()).unwrap_or(0),
                output_tokens: u32::try_from(u.output_tokens()).unwrap_or(0),
            });
            (Vec::new(), None, usage)
        }

        // Unknown / forward-compat variants.
        _ => (Vec::new(), None, None),
    }
}

/// Drive the Bedrock event stream to completion and emit `ChatChunk`s.
pub fn into_chat_stream(
    output: ConverseStreamOutput,
) -> BoxStream<'static, Result<ChatChunk, TakoError>> {
    use std::collections::HashMap;

    // The `stream` field is `pub`; move it out directly.
    let mut receiver = output.stream;
    let stream = async_stream::stream! {
        let mut tool_call_state: HashMap<i32, u32> = HashMap::new();
        let mut finish_reason: Option<FinishReason> = None;
        let mut usage = Usage::default();

        loop {
            match receiver.recv().await {
                Ok(Some(event)) => {
                    let (chunks, maybe_finish, maybe_usage) =
                        map_event(&event, &mut tool_call_state);
                    if let Some(r) = maybe_finish {
                        finish_reason = Some(r);
                    }
                    if let Some(u) = maybe_usage {
                        usage = u;
                    }
                    for chunk in chunks {
                        yield Ok(chunk);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("{e}") });
                    finish_reason = Some(FinishReason::Error);
                    break;
                }
            }
        }

        yield Ok(ChatChunk::End {
            finish_reason: finish_reason.unwrap_or(FinishReason::Other),
            usage,
        });
    };
    stream.boxed()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use aws_sdk_bedrockruntime::types::ToolUseBlockDelta;
    use aws_sdk_bedrockruntime::types::builders::{
        ContentBlockDeltaEventBuilder, ContentBlockStartEventBuilder, MessageStopEventBuilder,
    };
    use aws_sdk_bedrockruntime::types::{StopReason, TokenUsage};

    #[test]
    fn text_delta_emits_text_chunk() {
        let event = ContentBlockDeltaEventBuilder::default()
            .delta(br::ContentBlockDelta::Text("hi".into()))
            .content_block_index(0)
            .build()
            .unwrap();
        let mut state = std::collections::HashMap::new();
        let (chunks, finish, usage) = map_event(&Event::ContentBlockDelta(event), &mut state);
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            ChatChunk::Delta { text, tool_calls } => {
                assert_eq!(text.as_deref(), Some("hi"));
                assert!(tool_calls.is_empty());
            }
            other => panic!("expected Delta, got {other:?}"),
        }
        assert!(finish.is_none() && usage.is_none());
    }

    #[test]
    fn tool_use_start_then_delta_share_index() {
        let mut state = std::collections::HashMap::new();
        // ContentBlockStart::ToolUse for index 1
        let start = ContentBlockStartEventBuilder::default()
            .start(br::ContentBlockStart::ToolUse(
                br::ToolUseBlockStart::builder()
                    .tool_use_id("tu_a")
                    .name("search")
                    .build()
                    .unwrap(),
            ))
            .content_block_index(1)
            .build()
            .unwrap();
        let (chunks_start, _, _) = map_event(&Event::ContentBlockStart(start), &mut state);
        assert_eq!(chunks_start.len(), 1);
        let ChatChunk::Delta { tool_calls, .. } = &chunks_start[0] else {
            panic!("expected Delta");
        };
        assert_eq!(tool_calls[0].id.as_deref(), Some("tu_a"));
        assert_eq!(tool_calls[0].name.as_deref(), Some("search"));
        let start_index = tool_calls[0].index;

        // ContentBlockDelta::ToolUse for the same content_block_index
        let delta = ContentBlockDeltaEventBuilder::default()
            .delta(br::ContentBlockDelta::ToolUse(
                ToolUseBlockDelta::builder()
                    .input("{\"q\":\"tako\"}")
                    .build()
                    .unwrap(),
            ))
            .content_block_index(1)
            .build()
            .unwrap();
        let (chunks_delta, _, _) = map_event(&Event::ContentBlockDelta(delta), &mut state);
        let ChatChunk::Delta { tool_calls, .. } = &chunks_delta[0] else {
            panic!("expected Delta");
        };
        assert_eq!(tool_calls[0].index, start_index);
        assert_eq!(
            tool_calls[0].arguments_fragment.as_deref(),
            Some("{\"q\":\"tako\"}")
        );
    }

    #[test]
    fn message_stop_records_finish_reason() {
        let stop = MessageStopEventBuilder::default()
            .stop_reason(StopReason::EndTurn)
            .build()
            .unwrap();
        let mut state = std::collections::HashMap::new();
        let (_, finish, _) = map_event(&Event::MessageStop(stop), &mut state);
        assert_eq!(finish, Some(FinishReason::Stop));
    }

    #[test]
    fn message_stop_tool_use_maps_to_tool_calls() {
        let stop = MessageStopEventBuilder::default()
            .stop_reason(StopReason::ToolUse)
            .build()
            .unwrap();
        let mut state = std::collections::HashMap::new();
        let (_, finish, _) = map_event(&Event::MessageStop(stop), &mut state);
        assert_eq!(finish, Some(FinishReason::ToolCalls));
    }

    #[test]
    fn metadata_records_token_usage() {
        let meta = aws_sdk_bedrockruntime::types::ConverseStreamMetadataEvent::builder()
            .usage(
                TokenUsage::builder()
                    .input_tokens(7)
                    .output_tokens(3)
                    .total_tokens(10)
                    .build()
                    .unwrap(),
            )
            .build();
        let mut state = std::collections::HashMap::new();
        let (chunks, _, usage) = map_event(&Event::Metadata(meta), &mut state);
        assert!(chunks.is_empty());
        assert_eq!(
            usage,
            Some(Usage {
                input_tokens: 7,
                output_tokens: 3,
            })
        );
    }
}
