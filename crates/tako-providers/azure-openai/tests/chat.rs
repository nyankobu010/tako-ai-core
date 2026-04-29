//! End-to-end Azure OpenAI provider tests against wiremock.
//!
//! Verifies the Azure-specific URL shape (`/openai/deployments/{d}/chat/completions?api-version=...`)
//! and the `api-key` auth header. The wire body is identical to OpenAI, so
//! response-shape correctness is already covered by the OpenAI crate's tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::StreamExt;
use tako_core::{ChatChunk, ChatRequest, FinishReason, LlmProvider, Message, Principal};
use tako_providers_azure_openai::AzureOpenAiProvider;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn chat_uses_azure_url_shape_and_api_key_header() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "konnichiwa"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7},
    });
    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt-4o-prod/chat/completions"))
        .and(query_param("api-version", "2024-10-21"))
        .and(header("api-key", "test-azure-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = AzureOpenAiProvider::builder()
        .api_key("test-azure-key")
        .endpoint(server.uri())
        .deployment("gpt-4o-prod")
        .build()
        .unwrap();

    assert_eq!(p.id(), "azure-openai:gpt-4o-prod");

    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("", vec![Message::user("hi")]),
        )
        .await
        .unwrap();

    assert_eq!(resp.finish_reason, FinishReason::Stop);
    assert_eq!(resp.message.content[0].as_text(), Some("konnichiwa"));
}

#[tokio::test]
async fn custom_api_version_is_propagated() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1},
    });
    Mock::given(method("POST"))
        .and(path("/openai/deployments/d1/chat/completions"))
        .and(query_param("api-version", "2025-01-01-preview"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = AzureOpenAiProvider::builder()
        .api_key("k")
        .endpoint(server.uri())
        .deployment("d1")
        .api_version("2025-01-01-preview")
        .build()
        .unwrap();

    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("", vec![Message::user("hi")]),
        )
        .await
        .unwrap();
    assert_eq!(resp.message.content[0].as_text(), Some("ok"));
}

#[tokio::test]
async fn stream_terminates_with_end() {
    let server = MockServer::start().await;
    let chunks = [
        r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    ];
    let mut sse_body = String::new();
    for c in &chunks {
        sse_body.push_str("data: ");
        sse_body.push_str(c);
        sse_body.push_str("\n\n");
    }
    sse_body.push_str("data: [DONE]\n\n");

    Mock::given(method("POST"))
        .and(path("/openai/deployments/d1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("Content-Type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let p = AzureOpenAiProvider::builder()
        .api_key("k")
        .endpoint(server.uri())
        .deployment("d1")
        .build()
        .unwrap();

    let mut stream = p
        .stream(
            &Principal::anonymous(),
            ChatRequest::new("", vec![Message::user("hi")]),
        )
        .await
        .unwrap();

    let mut text = String::new();
    let mut saw_end = false;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ChatChunk::Delta { text: Some(t), .. } => text.push_str(&t),
            ChatChunk::End { finish_reason, .. } => {
                assert_eq!(finish_reason, FinishReason::Stop);
                saw_end = true;
            }
            _ => {}
        }
    }
    assert_eq!(text, "hi");
    assert!(saw_end);
}

#[tokio::test]
async fn provider_error_on_429_is_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/deployments/d1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&server)
        .await;

    let p = AzureOpenAiProvider::builder()
        .api_key("k")
        .endpoint(server.uri())
        .deployment("d1")
        .build()
        .unwrap();

    let err = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("", vec![Message::user("hi")]),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, tako_core::TakoError::RateLimited(_)));
}
