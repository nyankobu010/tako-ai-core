//! ChatChunk → OpenAI-shaped SSE delta mapper.
//!
//! Translates [`OrchEvent`] events into the JSON shape the official OpenAI
//! Python SDK expects from `client.chat.completions.create(stream=True)`:
//!
//! ```text
//! data: {"id":"...","object":"chat.completion.chunk","created":...,
//!        "model":"...","choices":[{"index":0,"delta":{"content":"hi"},
//!        "finish_reason":null}]}
//!
//! data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
//!        "usage":{...}}
//!
//! data: [DONE]
//! ```

use serde::Serialize;
use serde_json::{Value, json};
use tako_core::FinishReason;
use tako_orchestrator::OrchEvent;

#[derive(Debug, Serialize)]
struct OaChunkChoice {
    pub index: u32,
    pub delta: Value,
    pub finish_reason: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct OaChunk {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OaChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
}

fn finish_reason_str(r: FinishReason) -> &'static str {
    match r {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Error => "error",
        FinishReason::Other => "stop",
    }
}

/// Stable id used across all chunks of a single response.
pub fn new_chunk_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("chatcmpl-tako-{nanos}")
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Map a single [`OrchEvent`] to zero or more `data:` payload strings (no
/// trailing `\n\n`; the SSE writer appends those). The caller threads
/// `id` and `model` through every chunk so consumers see consistent ids.
pub fn event_to_payloads(event: &OrchEvent, id: &str, model: &str) -> Vec<String> {
    match event {
        OrchEvent::AssistantText { delta, .. } => {
            if delta.is_empty() {
                return Vec::new();
            }
            let chunk = OaChunk {
                id: id.to_string(),
                object: "chat.completion.chunk",
                created: now_secs(),
                model: model.to_string(),
                choices: vec![OaChunkChoice {
                    index: 0,
                    delta: json!({"role": "assistant", "content": delta}),
                    finish_reason: None,
                }],
                usage: None,
            };
            vec![serde_json::to_string(&chunk).unwrap_or_default()]
        }
        OrchEvent::ToolCallStart { name, id: tc_id, .. } => {
            let chunk = OaChunk {
                id: id.to_string(),
                object: "chat.completion.chunk",
                created: now_secs(),
                model: model.to_string(),
                choices: vec![OaChunkChoice {
                    index: 0,
                    delta: json!({
                        "tool_calls": [{
                            "index": 0,
                            "id": tc_id,
                            "type": "function",
                            "function": {"name": name, "arguments": ""}
                        }]
                    }),
                    finish_reason: None,
                }],
                usage: None,
            };
            vec![serde_json::to_string(&chunk).unwrap_or_default()]
        }
        OrchEvent::Final { output } => {
            let usage = json!({
                "prompt_tokens": output.usage.input_tokens,
                "completion_tokens": output.usage.output_tokens,
                "total_tokens": output.usage.input_tokens.saturating_add(output.usage.output_tokens),
            });
            let final_chunk = OaChunk {
                id: id.to_string(),
                object: "chat.completion.chunk",
                created: now_secs(),
                model: model.to_string(),
                choices: vec![OaChunkChoice {
                    index: 0,
                    delta: json!({}),
                    finish_reason: Some(finish_reason_str(FinishReason::Stop)),
                }],
                usage: Some(usage),
            };
            vec![serde_json::to_string(&final_chunk).unwrap_or_default()]
        }
        // StepStart and ToolCallResult don't have a clean OpenAI mapping —
        // OpenAI only surfaces tool-call deltas and final assistant text;
        // tool-result handling happens client-side after the stream ends.
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tako_core::Usage;
    use tako_orchestrator::OrchOutput;

    #[test]
    fn assistant_text_payload_has_content_delta() {
        let ev = OrchEvent::AssistantText {
            step: 0,
            delta: "hello".into(),
        };
        let payloads = event_to_payloads(&ev, "id-1", "gpt-test");
        assert_eq!(payloads.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&payloads[0]).unwrap();
        assert_eq!(v["choices"][0]["delta"]["content"], "hello");
        assert!(v["choices"][0]["finish_reason"].is_null());
        assert_eq!(v["model"], "gpt-test");
        assert_eq!(v["object"], "chat.completion.chunk");
    }

    #[test]
    fn empty_text_delta_emits_nothing() {
        let ev = OrchEvent::AssistantText {
            step: 0,
            delta: "".into(),
        };
        assert!(event_to_payloads(&ev, "id", "m").is_empty());
    }

    #[test]
    fn tool_call_start_emits_tool_calls_delta() {
        let ev = OrchEvent::ToolCallStart {
            step: 0,
            name: "search".into(),
            id: "call_42".into(),
        };
        let payloads = event_to_payloads(&ev, "id", "m");
        let v: serde_json::Value = serde_json::from_str(&payloads[0]).unwrap();
        let tc = &v["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(tc["function"]["name"], "search");
        assert_eq!(tc["id"], "call_42");
    }

    #[test]
    fn final_event_carries_usage_and_finish_reason() {
        let ev = OrchEvent::Final {
            output: Box::new(OrchOutput {
                text: "done".into(),
                message: tako_core::Message {
                    role: tako_core::Role::Assistant,
                    content: vec![tako_core::ContentPart::text("done")],
                },
                usage: Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                },
                steps: 1,
            }),
        };
        let payloads = event_to_payloads(&ev, "id", "m");
        let v: serde_json::Value = serde_json::from_str(&payloads[0]).unwrap();
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["prompt_tokens"], 7);
        assert_eq!(v["usage"]["completion_tokens"], 3);
        assert_eq!(v["usage"]["total_tokens"], 10);
    }
}
