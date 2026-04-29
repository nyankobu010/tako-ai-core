//! End-to-end Ollama provider tests against wiremock.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::StreamExt;
use tako_core::{
    ChatChunk, ChatRequest, ContentPart, FinishReason, LlmProvider, Message, Principal,
};
use tako_providers_ollama::OllamaProvider;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn chat_happy_path() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "model": "llama3",
        "created_at": "2026-04-29T00:00:00Z",
        "message": {"role": "assistant", "content": "hello there"},
        "done": true,
        "done_reason": "stop",
        "prompt_eval_count": 5,
        "eval_count": 2,
    });
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = OllamaProvider::builder()
        .base_url(server.uri())
        .model("llama3")
        .build()
        .unwrap();
    assert_eq!(p.id(), "ollama:llama3");

    let req = ChatRequest::new("llama3", vec![Message::user("hi")]);
    let resp = p.chat(&Principal::anonymous(), req).await.unwrap();
    assert_eq!(resp.finish_reason, FinishReason::Stop);
    assert_eq!(resp.usage.input_tokens, 5);
    assert_eq!(resp.usage.output_tokens, 2);
    assert_eq!(resp.message.content[0].as_text(), Some("hello there"));
}

#[tokio::test]
async fn no_auth_header_required() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "message": {"role": "assistant", "content": "ok"},
        "done": true,
    });
    // The request should NOT include an Authorization header.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = OllamaProvider::builder()
        .base_url(server.uri())
        .model("llama3")
        .build()
        .unwrap();
    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("llama3", vec![Message::user("hi")]),
        )
        .await
        .unwrap();
    assert_eq!(resp.message.content[0].as_text(), Some("ok"));
}

#[tokio::test]
async fn options_block_carries_temperature_and_num_predict() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "message": {"role": "assistant", "content": "ok"},
        "done": true,
    });
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_partial_json(serde_json::json!({
            "options": {
                "temperature": 0.5,
                "num_predict": 32,
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = OllamaProvider::builder()
        .base_url(server.uri())
        .model("llama3")
        .build()
        .unwrap();
    let mut req = ChatRequest::new("llama3", vec![Message::user("hi")]);
    req.temperature = Some(0.5);
    req.max_tokens = Some(32);
    let resp = p.chat(&Principal::anonymous(), req).await.unwrap();
    assert_eq!(resp.message.content[0].as_text(), Some("ok"));
}

#[tokio::test]
async fn provider_error_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream gone"))
        .mount(&server)
        .await;

    let p = OllamaProvider::builder()
        .base_url(server.uri())
        .model("llama3")
        .build()
        .unwrap();
    let err = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("llama3", vec![Message::user("hi")]),
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
async fn ndjson_stream_terminates_with_end() {
    let server = MockServer::start().await;
    // Three NDJSON frames: two with `done: false` and a final
    // `done: true` carrying token counts.
    let frames = [
        r#"{"message":{"role":"assistant","content":"hel"},"done":false}"#,
        r#"{"message":{"role":"assistant","content":"lo"},"done":false}"#,
        r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":3,"eval_count":2}"#,
    ];
    let body = format!("{}\n", frames.join("\n"));

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("Content-Type", "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let p = OllamaProvider::builder()
        .base_url(server.uri())
        .model("llama3")
        .build()
        .unwrap();

    let mut stream = p
        .stream(
            &Principal::anonymous(),
            ChatRequest::new("llama3", vec![Message::user("hi")]),
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
    // Ollama emits arguments as a JSON object, NOT a string — verify
    // that our adapter handles that.
    let body = serde_json::json!({
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "function": {
                    "name": "search",
                    "arguments": {"q": "tako"}
                }
            }]
        },
        "done": true,
        "done_reason": "stop",
        "prompt_eval_count": 4,
        "eval_count": 6,
    });
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = OllamaProvider::builder()
        .base_url(server.uri())
        .model("llama3")
        .build()
        .unwrap();
    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("llama3", vec![Message::user("search")]),
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
    // Synthesised id (Ollama doesn't issue them).
    assert_eq!(id, "ol_call_0");
    assert_eq!(name, "search");
    assert_eq!(args["q"], "tako");
}
