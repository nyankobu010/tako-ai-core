//! End-to-end Mistral provider tests against wiremock.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::StreamExt;
use tako_core::{
    ChatChunk, ChatRequest, ContentPart, FinishReason, LlmProvider, Message, Principal,
};
use tako_providers_mistral::MistralProvider;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn chat_happy_path() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "id": "x",
        "object": "chat.completion",
        "created": 0,
        "model": "mistral-large-latest",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "bonjour"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10},
    });
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = MistralProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("mistral-large-latest")
        .build()
        .unwrap();

    assert_eq!(p.id(), "mistral:mistral-large-latest");

    let req = ChatRequest::new("mistral-large-latest", vec![Message::user("salut")]);
    let resp = p.chat(&Principal::anonymous(), req).await.unwrap();
    assert_eq!(resp.finish_reason, FinishReason::Stop);
    assert_eq!(resp.usage.input_tokens, 7);
    assert_eq!(resp.usage.output_tokens, 3);
    assert_eq!(resp.message.content[0].as_text(), Some("bonjour"));
}

#[tokio::test]
async fn vendor_extensions_are_serialised() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    });
    // Assert that safe_prompt + random_seed end up in the request body.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(serde_json::json!({
            "safe_prompt": true,
            "random_seed": 42,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = MistralProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("mistral-large-latest")
        .safe_prompt(true)
        .random_seed(42)
        .build()
        .unwrap();
    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("mistral-large-latest", vec![Message::user("hi")]),
        )
        .await
        .unwrap();
    assert_eq!(resp.message.content[0].as_text(), Some("ok"));
}

#[tokio::test]
async fn chat_returns_provider_error_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream gone"))
        .mount(&server)
        .await;

    let p = MistralProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("mistral-large-latest")
        .build()
        .unwrap();

    let err = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("mistral-large-latest", vec![Message::user("hi")]),
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
async fn provider_error_on_429_is_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&server)
        .await;

    let p = MistralProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("mistral-large-latest")
        .build()
        .unwrap();

    let err = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("mistral-large-latest", vec![Message::user("hi")]),
        )
        .await
        .unwrap_err();
    use tako_core::TakoError;
    assert!(matches!(err, TakoError::RateLimited(_)));
}

#[tokio::test]
async fn stream_terminates_with_end() {
    let server = MockServer::start().await;
    let chunks = [
        r#"{"choices":[{"delta":{"content":"bon"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"content":"jour"},"finish_reason":null}]}"#,
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

    let p = MistralProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("mistral-large-latest")
        .build()
        .unwrap();

    let mut stream = p
        .stream(
            &Principal::anonymous(),
            ChatRequest::new("mistral-large-latest", vec![Message::user("hi")]),
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
    assert_eq!(text, "bonjour");
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

    let p = MistralProvider::builder()
        .api_key("test")
        .base_url(server.uri())
        .model("mistral-large-latest")
        .build()
        .unwrap();

    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("mistral-large-latest", vec![Message::user("search")]),
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
