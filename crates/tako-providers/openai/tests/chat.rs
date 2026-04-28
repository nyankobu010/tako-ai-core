//! End-to-end OpenAI provider tests against wiremock.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::StreamExt;
use tako_core::{
    ChatChunk, ChatRequest, ContentPart, FinishReason, LlmProvider, Message, Principal,
};
use tako_providers_openai::OpenAiProvider;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn chat_happy_path() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "id": "x",
        "object": "chat.completion",
        "created": 0,
        "model": "gpt-test",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "hello there"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7},
    });
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = OpenAiProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("gpt-test")
        .build()
        .unwrap();

    let req = ChatRequest::new("gpt-test", vec![Message::user("hi")]);
    let resp = p.chat(&Principal::anonymous(), req).await.unwrap();
    assert_eq!(resp.finish_reason, FinishReason::Stop);
    assert_eq!(resp.usage.input_tokens, 5);
    assert_eq!(resp.usage.output_tokens, 2);
    assert_eq!(resp.message.content[0].as_text(), Some("hello there"));
}

#[tokio::test]
async fn chat_returns_provider_error_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream gone"))
        .mount(&server)
        .await;

    let p = OpenAiProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("gpt-test")
        .build()
        .unwrap();

    let err = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("gpt-test", vec![Message::user("hi")]),
        )
        .await
        .unwrap_err();

    use tako_core::TakoError;
    let TakoError::Provider { details, .. } = &err else {
        panic!("got {err:?}");
    };
    assert_eq!(details.status_code, Some(503));
    assert!(err.is_transient());
}

#[tokio::test]
async fn stream_terminates_with_end() {
    let server = MockServer::start().await;
    let chunks = [
        r#"{"choices":[{"delta":{"content":"hel"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"content":"lo"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#,
    ];
    let mut body = String::new();
    for c in &chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("Content-Type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let p = OpenAiProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("gpt-test")
        .build()
        .unwrap();

    let mut stream = p
        .stream(
            &Principal::anonymous(),
            ChatRequest::new("gpt-test", vec![Message::user("hi")]),
        )
        .await
        .unwrap();

    let mut text = String::new();
    let mut saw_end = false;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ChatChunk::Delta { text: Some(t), .. } => text.push_str(&t),
            ChatChunk::End {
                finish_reason,
                usage,
            } => {
                assert_eq!(finish_reason, FinishReason::Stop);
                assert_eq!(usage.input_tokens, 3);
                assert_eq!(usage.output_tokens, 2);
                saw_end = true;
            }
            _ => {}
        }
    }
    assert_eq!(text, "hello");
    assert!(saw_end, "stream did not yield ChatChunk::End");
}

#[tokio::test]
async fn tool_calls_round_trip() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "search", "arguments": "{\"q\":\"tako\"}"}
                }]
            },
            "finish_reason": "tool_calls",
        }],
        "usage": {"prompt_tokens": 4, "completion_tokens": 6, "total_tokens": 10},
    });
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = OpenAiProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("gpt-test")
        .build()
        .unwrap();

    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("gpt-test", vec![Message::user("search")]),
        )
        .await
        .unwrap();
    assert_eq!(resp.finish_reason, FinishReason::ToolCalls);
    let tool_call = resp
        .message
        .content
        .iter()
        .find(|c| matches!(c, ContentPart::ToolCall { .. }))
        .expect("expected ToolCall");
    let ContentPart::ToolCall { id, name, args } = tool_call else {
        unreachable!()
    };
    assert_eq!(id, "call_1");
    assert_eq!(name, "search");
    assert_eq!(args["q"], "tako");
}
