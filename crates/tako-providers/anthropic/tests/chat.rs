//! End-to-end Anthropic provider tests against wiremock.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::StreamExt;
use tako_core::{ChatChunk, ChatRequest, ContentPart, FinishReason, LlmProvider, Message, Principal};
use tako_providers_anthropic::AnthropicProvider;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn chat_happy_path() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "id": "msg_x",
        "type": "message",
        "role": "assistant",
        "content": [{"type":"text","text":"hello there"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = AnthropicProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("claude-test")
        .build()
        .unwrap();

    let req = ChatRequest::new("claude-test", vec![Message::user("hi")]);
    let resp = p.chat(&Principal::anonymous(), req).await.unwrap();
    assert_eq!(resp.finish_reason, FinishReason::Stop);
    assert_eq!(resp.usage.input_tokens, 5);
    assert_eq!(resp.message.content[0].as_text(), Some("hello there"));
}

#[tokio::test]
async fn tool_use_response() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "id": "msg_x",
        "content": [
            {"type":"tool_use","id":"toolu_01","name":"search","input":{"q":"tako"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 4, "output_tokens": 6}
    });
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = AnthropicProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("claude-test")
        .build()
        .unwrap();

    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("claude-test", vec![Message::user("search")]),
        )
        .await
        .unwrap();
    assert_eq!(resp.finish_reason, FinishReason::ToolCalls);
    let ContentPart::ToolCall { id, name, args } = resp.message.content.iter().find(|c| matches!(c, ContentPart::ToolCall { .. })).unwrap() else {
        unreachable!()
    };
    assert_eq!(id, "toolu_01");
    assert_eq!(name, "search");
    assert_eq!(args["q"], "tako");
}

#[tokio::test]
async fn stream_terminates_with_end() {
    let server = MockServer::start().await;
    let events = [
        r#"{"type":"message_start","message":{"id":"x","type":"message","role":"assistant","content":[],"usage":{"input_tokens":5,"output_tokens":0}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hel"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":5,"output_tokens":2}}"#,
        r#"{"type":"message_stop"}"#,
    ];
    let mut body = String::new();
    for e in &events {
        let v: serde_json::Value = serde_json::from_str(e).unwrap();
        body.push_str(&format!("event: {}\n", v["type"].as_str().unwrap()));
        body.push_str(&format!("data: {e}\n\n"));
    }

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("Content-Type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let p = AnthropicProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("claude-test")
        .build()
        .unwrap();

    let mut stream = p
        .stream(
            &Principal::anonymous(),
            ChatRequest::new("claude-test", vec![Message::user("hi")]),
        )
        .await
        .unwrap();

    let mut text = String::new();
    let mut saw_end = false;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ChatChunk::Delta { text: Some(t), .. } => text.push_str(&t),
            ChatChunk::End { finish_reason, usage } => {
                assert_eq!(finish_reason, FinishReason::Stop);
                assert_eq!(usage.input_tokens, 5);
                assert_eq!(usage.output_tokens, 2);
                saw_end = true;
            }
            _ => {}
        }
    }
    assert_eq!(text, "hello");
    assert!(saw_end);
}
