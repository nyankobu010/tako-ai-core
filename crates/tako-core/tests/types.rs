//! Round-trip serde + invariant tests for `tako-core` types.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tako_core::{
    ChatChunk, ChatRequest, ContentPart, FinishReason, Message, Principal, Role, ToolCallDelta, ToolSchema, Usage,
};

#[test]
fn principal_roundtrip() {
    let p = Principal::new("acme", "alice");
    let s = serde_json::to_string(&p).unwrap();
    let back: Principal = serde_json::from_str(&s).unwrap();
    assert_eq!(p, back);
}

#[test]
fn principal_anonymous() {
    let p = Principal::anonymous();
    assert_eq!(p.tenant_id, "anonymous");
    assert!(p.roles.is_empty());
}

#[test]
fn role_serialises_lowercase() {
    let s = serde_json::to_string(&Role::Assistant).unwrap();
    assert_eq!(s, "\"assistant\"");
}

#[test]
fn message_constructors() {
    let m = Message::user("hello");
    assert_eq!(m.role, Role::User);
    assert_eq!(m.content[0].as_text(), Some("hello"));
}

#[test]
fn content_part_tagged_serialisation() {
    let cp = ContentPart::ToolCall {
        id: "call_1".into(),
        name: "search".into(),
        args: serde_json::json!({"q": "tako"}),
    };
    let v: serde_json::Value = serde_json::to_value(&cp).unwrap();
    assert_eq!(v["type"], "tool_call");
    assert_eq!(v["name"], "search");
    let back: ContentPart = serde_json::from_value(v).unwrap();
    assert_eq!(back, cp);
}

#[test]
fn chat_request_default_skips() {
    let req = ChatRequest::new("model", vec![Message::user("hi")]);
    let v: serde_json::Value = serde_json::to_value(&req).unwrap();
    // tools / temperature / max_tokens / stop / metadata all default and
    // should be skipped.
    assert!(!v.as_object().unwrap().contains_key("tools"));
    assert!(!v.as_object().unwrap().contains_key("temperature"));
    assert!(!v.as_object().unwrap().contains_key("max_tokens"));
    assert!(!v.as_object().unwrap().contains_key("stop"));
    assert!(!v.as_object().unwrap().contains_key("metadata"));
}

#[test]
fn chat_chunk_streaming_contract() {
    let delta = ChatChunk::Delta {
        text: Some("hello".into()),
        tool_calls: vec![],
    };
    let end = ChatChunk::End {
        finish_reason: FinishReason::Stop,
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
        },
    };
    // Both must round-trip cleanly.
    for chunk in [&delta, &end] {
        let s = serde_json::to_string(chunk).unwrap();
        let back: ChatChunk = serde_json::from_str(&s).unwrap();
        assert_eq!(&back, chunk);
    }
}

#[test]
fn tool_call_delta_index() {
    let d = ToolCallDelta {
        index: 0,
        id: Some("call_1".into()),
        name: Some("search".into()),
        arguments_fragment: Some("{\"q\":".into()),
    };
    let s = serde_json::to_string(&d).unwrap();
    let back: ToolCallDelta = serde_json::from_str(&s).unwrap();
    assert_eq!(back, d);
}

#[test]
fn usage_total_saturates() {
    let u = Usage {
        input_tokens: u32::MAX,
        output_tokens: 5,
    };
    assert_eq!(u.total(), u32::MAX);
}

#[test]
fn tool_schema_optional_annotations() {
    let t = ToolSchema {
        name: "echo".into(),
        description: "Echo input.".into(),
        input_schema: serde_json::json!({"type": "object"}),
        annotations: None,
    };
    let s = serde_json::to_string(&t).unwrap();
    assert!(!s.contains("annotations"));
}
