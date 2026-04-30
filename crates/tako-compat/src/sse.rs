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
        OrchEvent::ToolCallStart {
            name, id: tc_id, ..
        } => {
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

/// Phase 9.C — map a single [`OrchEvent`] to zero or more named
/// `tako.*` SSE extensions, returned as `(event_name, json_payload)`
/// pairs. The caller emits each as
/// `axum::response::sse::Event::default().event(name).data(payload)`,
/// producing SSE frames with both an `event:` line and a `data:` line.
/// OpenAI clients ignore unknown event names per the SSE spec, so this
/// is a zero-impact additive sidechannel for tako-aware consumers.
///
/// Currently emits:
///
/// - [`OrchEvent::VerifierScore`] →
///   `("tako.verifier_score", "{"step":N,"branch":B,"score":S}")`
///   (Phase 9.C)
/// - [`OrchEvent::Recursion`] →
///   `("tako.recursion", "{"depth":D,"confidence":C}")`
///   (Phase 9.C)
/// - [`OrchEvent::ToolCallStart`] →
///   `("tako.tool_call_start", "{"step":N,"name":"...","id":"..."}")`
///   (Phase 10.B). Emitted in addition to the existing OpenAI
///   `tool_calls` delta in [`event_to_payloads`] — OpenAI clients
///   ignore the named extension; tako-aware consumers gain a typed
///   handle on the start of each tool invocation.
/// - [`OrchEvent::ToolCallResult`] →
///   `("tako.tool_call_result",
///   "{"step":N,"id":"...","result":<json>,"is_error":<bool>}")`
///   (Phase 10.B). Closes the gap where this variant had no OpenAI
///   mapping at all and was silently dropped — tool results are now
///   observable mid-stream.
///
/// All other variants return an empty `Vec` (no extension emitted —
/// the variant is either part of the OpenAI mapping or has no
/// out-of-band signalling need yet).
pub fn event_to_tako_extensions(event: &OrchEvent) -> Vec<(&'static str, String)> {
    match event {
        OrchEvent::VerifierScore {
            step,
            branch,
            score,
        } => {
            let body = json!({
                "step": step,
                "branch": branch,
                "score": score,
            });
            vec![("tako.verifier_score", body.to_string())]
        }
        OrchEvent::Recursion { depth, confidence } => {
            let body = json!({
                "depth": depth,
                "confidence": confidence,
            });
            vec![("tako.recursion", body.to_string())]
        }
        OrchEvent::ToolCallStart {
            step,
            name,
            id: tc_id,
        } => {
            let body = json!({
                "step": step,
                "name": name,
                "id": tc_id,
            });
            vec![("tako.tool_call_start", body.to_string())]
        }
        OrchEvent::ToolCallResult {
            step,
            id: tc_id,
            result,
            is_error,
        } => {
            let body = json!({
                "step": step,
                "id": tc_id,
                "result": result,
                "is_error": is_error,
            });
            vec![("tako.tool_call_result", body.to_string())]
        }
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
    fn verifier_score_emits_tako_named_extension() {
        // Score uses an exactly-representable f32 value to avoid
        // float-widening surprises in the JSON round-trip.
        let ev = OrchEvent::VerifierScore {
            step: 3,
            branch: 1,
            score: 0.5,
        };
        let exts = event_to_tako_extensions(&ev);
        assert_eq!(exts.len(), 1);
        let (name, payload) = &exts[0];
        assert_eq!(*name, "tako.verifier_score");
        let v: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(v["step"], 3);
        assert_eq!(v["branch"], 1);
        assert_eq!(v["score"].as_f64().unwrap(), 0.5);
        // `event_to_payloads` (the OpenAI mapping) must NOT emit
        // anything for this variant — extension goes through the
        // sidechannel only.
        assert!(event_to_payloads(&ev, "id", "m").is_empty());
    }

    #[test]
    fn recursion_emits_tako_named_extension() {
        // Confidence uses an exactly-representable f32 value.
        let ev = OrchEvent::Recursion {
            depth: 2,
            confidence: 0.25,
        };
        let exts = event_to_tako_extensions(&ev);
        assert_eq!(exts.len(), 1);
        let (name, payload) = &exts[0];
        assert_eq!(*name, "tako.recursion");
        let v: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(v["depth"], 2);
        assert_eq!(v["confidence"].as_f64().unwrap(), 0.25);
        assert!(event_to_payloads(&ev, "id", "m").is_empty());
    }

    #[test]
    fn opaque_variants_emit_no_tako_extensions() {
        // Variants that already have a clean OpenAI mapping (and no
        // tako-side metadata that OpenAI cannot represent) must not
        // also emit a tako-named frame — the extension sidechannel is
        // reserved for events OpenAI cannot represent and for the
        // tool-call lifecycle (Phase 10.B), which gains a parallel
        // tako-named frame for tako-aware consumers.
        let cases = [
            OrchEvent::AssistantText {
                step: 0,
                delta: "hi".into(),
            },
            OrchEvent::StepStart { step: 0 },
        ];
        for ev in &cases {
            assert!(
                event_to_tako_extensions(ev).is_empty(),
                "unexpected tako extension for {ev:?}",
            );
        }
    }

    #[test]
    fn tool_call_start_emits_named_tako_extension() {
        // Phase 10.B — `ToolCallStart` now has both an OpenAI
        // `tool_calls` delta in `event_to_payloads` AND a
        // `tako.tool_call_start` named extension. OpenAI clients
        // ignore unknown event names per the SSE spec, so the dual
        // emission is zero-impact for them.
        let ev = OrchEvent::ToolCallStart {
            step: 1,
            name: "search".into(),
            id: "tc-abc".into(),
        };
        let exts = event_to_tako_extensions(&ev);
        assert_eq!(exts.len(), 1);
        let (name, payload) = &exts[0];
        assert_eq!(*name, "tako.tool_call_start");
        let v: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(v["step"], 1);
        assert_eq!(v["name"], "search");
        assert_eq!(v["id"], "tc-abc");
        // The OpenAI mapping for `ToolCallStart` is unchanged — still
        // emits exactly one chat.completion.chunk with a `tool_calls`
        // delta.
        assert_eq!(event_to_payloads(&ev, "id-1", "gpt-test").len(), 1);
    }

    #[test]
    fn tool_call_result_emits_named_tako_extension() {
        // Phase 10.B — `ToolCallResult` had no OpenAI mapping (silently
        // dropped at the `_ => Vec::new()` arm of `event_to_payloads`)
        // so tako-aware clients never saw tool results mid-stream. The
        // `tako.tool_call_result` named extension closes that gap.
        let ev = OrchEvent::ToolCallResult {
            step: 1,
            id: "tc-abc".into(),
            result: serde_json::json!({"ok": true, "rows": 3}),
            is_error: false,
        };
        let exts = event_to_tako_extensions(&ev);
        assert_eq!(exts.len(), 1);
        let (name, payload) = &exts[0];
        assert_eq!(*name, "tako.tool_call_result");
        let v: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(v["step"], 1);
        assert_eq!(v["id"], "tc-abc");
        assert_eq!(v["result"]["ok"], true);
        assert_eq!(v["result"]["rows"], 3);
        assert_eq!(v["is_error"], false);
        // Still no OpenAI mapping for this variant.
        assert!(event_to_payloads(&ev, "id-1", "gpt-test").is_empty());
    }

    #[test]
    fn tool_call_result_propagates_is_error_true() {
        // Errors from a tool surface with `is_error: true` so consumers
        // can short-circuit downstream processing.
        let ev = OrchEvent::ToolCallResult {
            step: 2,
            id: "tc-xyz".into(),
            result: serde_json::json!({"error": "rate limited"}),
            is_error: true,
        };
        let exts = event_to_tako_extensions(&ev);
        let (_, payload) = &exts[0];
        let v: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(v["is_error"], true);
        assert_eq!(v["result"]["error"], "rate limited");
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
