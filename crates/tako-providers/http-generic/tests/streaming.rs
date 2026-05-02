//! Phase 11.B — integration tests for `HttpGenericProvider::stream`
//! against a `wiremock` server emitting both OpenAI-compat SSE and
//! NDJSON shapes. Covers the JSON-Pointer extraction, capability
//! gating, and error-propagation behaviour.

#![allow(clippy::unwrap_used, clippy::panic)]

use futures::StreamExt;
use serde_json::json;
use tako_core::{ChatChunk, ChatRequest, FinishReason, LlmProvider, Message, Principal, Usage};
use tako_providers_http_generic::{HttpGenericConfig, HttpGenericProvider, StreamConfig};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn principal() -> Principal {
    Principal::anonymous()
}

fn req(model: &str) -> ChatRequest {
    ChatRequest::new(model, vec![Message::user("hi")])
}

async fn collect(
    mut s: futures::stream::BoxStream<'static, Result<ChatChunk, tako_core::TakoError>>,
) -> Vec<ChatChunk> {
    let mut out = Vec::new();
    while let Some(item) = s.next().await {
        out.push(item.unwrap());
    }
    out
}

fn openai_sse_provider(url: String) -> HttpGenericProvider {
    HttpGenericProvider::new(HttpGenericConfig {
        id: "test".into(),
        model: "m".into(),
        url,
        body_template: json!({"model": "{{ model }}"}),
        response_text_pointer: "/text".into(),
        stream_config: Some(StreamConfig::OpenAiSse {
            content_pointer: "/choices/0/delta/content".into(),
            finish_reason_pointer: "/choices/0/finish_reason".into(),
            usage_pointer: Some("/usage".into()),
        }),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn openai_sse_two_deltas_then_done_yields_delta_delta_end() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n\
                data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
                data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let p = openai_sse_provider(server.uri());
    let chunks = collect(p.stream(&principal(), req("m")).await.unwrap()).await;
    let texts: Vec<_> = chunks
        .iter()
        .filter_map(|c| match c {
            ChatChunk::Delta { text, .. } => text.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["hel".to_string(), "lo".to_string()]);
    assert!(matches!(chunks.last().unwrap(), ChatChunk::End { .. }));
}

#[tokio::test]
async fn openai_sse_finish_reason_extracted() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n\
                data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n\
                data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let p = openai_sse_provider(server.uri());
    let chunks = collect(p.stream(&principal(), req("m")).await.unwrap()).await;
    match chunks.last().unwrap() {
        ChatChunk::End { finish_reason, .. } => {
            assert_eq!(*finish_reason, FinishReason::Length)
        }
        other => panic!("expected End, got {other:?}"),
    }
}

#[tokio::test]
async fn openai_sse_usage_pointer_resolved() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n\
                data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"input_tokens\":12,\"output_tokens\":7}}\n\n\
                data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let p = openai_sse_provider(server.uri());
    let chunks = collect(p.stream(&principal(), req("m")).await.unwrap()).await;
    match chunks.last().unwrap() {
        ChatChunk::End { usage, .. } => {
            assert_eq!(
                *usage,
                Usage {
                    input_tokens: 12,
                    output_tokens: 7,
                }
            );
        }
        other => panic!("expected End, got {other:?}"),
    }
}

#[tokio::test]
async fn openai_sse_invalid_frame_yields_error_chunk_and_continues() {
    let server = MockServer::start().await;
    let body = "data: not-json\n\n\
                data: {\"choices\":[{\"delta\":{\"content\":\"after\"}}]}\n\n\
                data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let p = openai_sse_provider(server.uri());
    let chunks = collect(p.stream(&principal(), req("m")).await.unwrap()).await;
    let n_errors = chunks
        .iter()
        .filter(|c| matches!(c, ChatChunk::Error { .. }))
        .count();
    let n_deltas = chunks
        .iter()
        .filter(|c| matches!(c, ChatChunk::Delta { .. }))
        .count();
    assert_eq!(n_errors, 1, "expected 1 error chunk");
    assert_eq!(n_deltas, 1, "expected 1 delta after the error");
    assert!(matches!(chunks.last().unwrap(), ChatChunk::End { .. }));
}

#[tokio::test]
async fn ndjson_two_lines_then_finish_field_yields_delta_delta_end() {
    let server = MockServer::start().await;
    let body = "{\"text\":\"foo\"}\n{\"text\":\"bar\",\"done\":\"stop\"}\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/x-ndjson")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let p = HttpGenericProvider::new(HttpGenericConfig {
        id: "test".into(),
        model: "m".into(),
        url: server.uri(),
        body_template: json!({"model": "{{ model }}"}),
        response_text_pointer: "/text".into(),
        stream_config: Some(StreamConfig::NdJson {
            content_pointer: "/text".into(),
            finish_reason_pointer: "/done".into(),
            usage_pointer: None,
        }),
        ..Default::default()
    })
    .unwrap();
    let chunks = collect(p.stream(&principal(), req("m")).await.unwrap()).await;
    let texts: Vec<_> = chunks
        .iter()
        .filter_map(|c| match c {
            ChatChunk::Delta { text, .. } => text.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["foo".to_string(), "bar".to_string()]);
    match chunks.last().unwrap() {
        ChatChunk::End { finish_reason, .. } => assert_eq!(*finish_reason, FinishReason::Stop),
        other => panic!("expected End, got {other:?}"),
    }
}

#[tokio::test]
async fn ndjson_terminates_on_eof_without_finish_reason() {
    let server = MockServer::start().await;
    let body = "{\"text\":\"foo\"}\n{\"text\":\"bar\"}\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/x-ndjson")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let p = HttpGenericProvider::new(HttpGenericConfig {
        id: "test".into(),
        model: "m".into(),
        url: server.uri(),
        body_template: json!({"model": "{{ model }}"}),
        response_text_pointer: "/text".into(),
        stream_config: Some(StreamConfig::NdJson {
            content_pointer: "/text".into(),
            finish_reason_pointer: "/done".into(),
            usage_pointer: None,
        }),
        ..Default::default()
    })
    .unwrap();
    let chunks = collect(p.stream(&principal(), req("m")).await.unwrap()).await;
    assert_eq!(
        chunks
            .iter()
            .filter(|c| matches!(c, ChatChunk::Delta { .. }))
            .count(),
        2
    );
    match chunks.last().unwrap() {
        ChatChunk::End { finish_reason, .. } => assert_eq!(*finish_reason, FinishReason::Other),
        other => panic!("expected End, got {other:?}"),
    }
}

#[tokio::test]
async fn stream_without_stream_config_returns_invalid_error() {
    let p = HttpGenericProvider::new(HttpGenericConfig {
        id: "test".into(),
        model: "m".into(),
        url: "https://example.invalid".into(),
        body_template: json!({}),
        response_text_pointer: "/text".into(),
        stream_config: None,
        ..Default::default()
    })
    .unwrap();
    let result = p.stream(&principal(), req("m")).await;
    match result {
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains("stream_config"),
                "expected message to mention stream_config, got: {msg}"
            );
        }
        Ok(_) => panic!("expected error when stream_config is None"),
    }
}

#[tokio::test]
async fn non_2xx_streaming_response_returns_provider_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream busy"))
        .mount(&server)
        .await;
    let p = openai_sse_provider(server.uri());
    let result = p.stream(&principal(), req("m")).await;
    match result {
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains("503"),
                "expected 503 in error message, got: {msg}"
            );
        }
        Ok(_) => panic!("expected error on 503 response"),
    }
}

#[tokio::test]
async fn stream_does_not_panic_when_pointer_is_unresolvable() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    // Pointer that does not resolve in any frame.
    let p = HttpGenericProvider::new(HttpGenericConfig {
        id: "test".into(),
        model: "m".into(),
        url: server.uri(),
        body_template: json!({}),
        response_text_pointer: "/text".into(),
        stream_config: Some(StreamConfig::OpenAiSse {
            content_pointer: "/no/such/path".into(),
            finish_reason_pointer: "/no/finish".into(),
            usage_pointer: None,
        }),
        ..Default::default()
    })
    .unwrap();
    let chunks = collect(p.stream(&principal(), req("m")).await.unwrap()).await;
    assert_eq!(
        chunks
            .iter()
            .filter(|c| matches!(c, ChatChunk::Delta { .. }))
            .count(),
        0
    );
    assert!(matches!(chunks.last().unwrap(), ChatChunk::End { .. }));
}
