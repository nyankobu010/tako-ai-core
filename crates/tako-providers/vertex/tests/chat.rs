//! End-to-end Vertex AI provider tests against wiremock.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::StreamExt;
use tako_core::{
    ChatChunk, ChatRequest, ContentPart, FinishReason, LlmProvider, Message, Principal,
};
use tako_providers_vertex::VertexProvider;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_provider(server_uri: &str) -> VertexProvider {
    VertexProvider::builder()
        .access_token("ya29.test")
        .project_id("my-proj")
        .location("us-central1")
        .model("gemini-2.0-pro")
        .endpoint_url(server_uri)
        .build()
        .unwrap()
}

#[tokio::test]
async fn chat_uses_generate_content_url_and_bearer_auth() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{"text": "konnichiwa"}]
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 2}
    });
    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/my-proj/locations/us-central1/publishers/google/models/gemini-2.0-pro:generateContent",
        ))
        .and(header("Authorization", "Bearer ya29.test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let p = build_provider(&server.uri());
    assert_eq!(p.id(), "vertex:gemini-2.0-pro");

    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("gemini-2.0-pro", vec![Message::user("hi")]),
        )
        .await
        .unwrap();

    assert_eq!(resp.finish_reason, FinishReason::Stop);
    assert_eq!(resp.usage.input_tokens, 5);
    assert_eq!(resp.usage.output_tokens, 2);
    assert_eq!(resp.message.content[0].as_text(), Some("konnichiwa"));
}

#[tokio::test]
async fn system_message_hoisted_to_system_instruction() {
    let server = MockServer::start().await;
    let canned = serde_json::json!({
        "candidates": [{
            "content": {"role": "model", "parts": [{"text": "ok"}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
    });
    // Verify the request body has systemInstruction (not a system role in
    // contents) — Vertex hoists system messages.
    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/my-proj/locations/us-central1/publishers/google/models/gemini-2.0-pro:generateContent",
        ))
        .and(body_partial_json(serde_json::json!({
            "systemInstruction": {"parts": [{"text": "be brief"}]}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(canned))
        .mount(&server)
        .await;

    let p = build_provider(&server.uri());
    let req = ChatRequest::new(
        "gemini-2.0-pro",
        vec![Message::system("be brief"), Message::user("hi")],
    );
    let resp = p.chat(&Principal::anonymous(), req).await.unwrap();
    assert_eq!(resp.message.content[0].as_text(), Some("ok"));
}

#[tokio::test]
async fn function_call_response_maps_to_tool_call() {
    let server = MockServer::start().await;
    let canned = serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{
                    "functionCall": {
                        "name": "search",
                        "args": {"q": "tako"}
                    }
                }]
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 4, "candidatesTokenCount": 6}
    });
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(canned))
        .mount(&server)
        .await;

    let p = build_provider(&server.uri());
    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("gemini-2.0-pro", vec![Message::user("search")]),
        )
        .await
        .unwrap();

    // STOP + tool call should be promoted to ToolCalls finish.
    assert_eq!(resp.finish_reason, FinishReason::ToolCalls);
    let tool_call = resp
        .message
        .content
        .iter()
        .find(|c| matches!(c, ContentPart::ToolCall { .. }))
        .expect("expected ToolCall");
    let ContentPart::ToolCall { name, args, .. } = tool_call else {
        unreachable!()
    };
    assert_eq!(name, "search");
    assert_eq!(args["q"], "tako");
}

#[tokio::test]
async fn stream_terminates_with_end() {
    let server = MockServer::start().await;
    let chunks = [
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hel"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"lo"}]}}]}"#,
        r#"{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}}"#,
    ];
    let mut sse_body = String::new();
    for c in &chunks {
        sse_body.push_str("data: ");
        sse_body.push_str(c);
        sse_body.push_str("\n\n");
    }

    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/my-proj/locations/us-central1/publishers/google/models/gemini-2.0-pro:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("Content-Type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let p = build_provider(&server.uri());
    let mut stream = p
        .stream(
            &Principal::anonymous(),
            ChatRequest::new("gemini-2.0-pro", vec![Message::user("hi")]),
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
    assert!(saw_end);
}

/// Helper — pull every `ToolCallDelta` out of the stream so tests
/// can assert on `id` / `arguments_fragment` without re-implementing
/// the chunk loop each time. Phase 48.
async fn collect_stream_tool_calls(p: &VertexProvider) -> Vec<tako_core::ToolCallDelta> {
    let mut stream = p
        .stream(
            &Principal::anonymous(),
            ChatRequest::new("gemini-2.0-pro", vec![Message::user("ask")]),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        if let Ok(ChatChunk::Delta { tool_calls, .. }) = item {
            out.extend(tool_calls);
        }
    }
    out
}

#[tokio::test]
async fn stream_tool_call_id_matches_non_streaming() {
    // Phase 48 — the same (name, args) on the streaming path
    // must produce the SAME id as the non-streaming
    // `from_vertex_response` would, so operators can correlate
    // a streamed call with a later non-streamed retry.
    let server = MockServer::start().await;

    // SSE: one chunk with a function_call, then a STOP terminator.
    let chunks = [
        r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"city":"Tokyo"}}}]}}]}"#,
        r#"{"candidates":[{"finishReason":"STOP"}]}"#,
    ];
    let mut sse_body = String::new();
    for c in &chunks {
        sse_body.push_str("data: ");
        sse_body.push_str(c);
        sse_body.push_str("\n\n");
    }
    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/my-proj/locations/us-central1/publishers/google/models/gemini-2.0-pro:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("Content-Type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    // Same payload via the non-streaming `chat()`.
    let non_stream_body = serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{"functionCall": {"name": "lookup", "args": {"city": "Tokyo"}}}]
            },
            "finishReason": "STOP"
        }]
    });
    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/my-proj/locations/us-central1/publishers/google/models/gemini-2.0-pro:generateContent",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(non_stream_body))
        .mount(&server)
        .await;

    let p = build_provider(&server.uri());

    // Streamed path — extract the `id` from the streamed delta.
    let deltas = collect_stream_tool_calls(&p).await;
    assert_eq!(
        deltas.len(),
        1,
        "expected exactly one streamed tool-call delta"
    );
    let streamed_id = deltas[0].id.clone().expect("streamed delta has id");

    // Non-streaming path — extract the `id` from the parsed `ToolCall`.
    let resp = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("gemini-2.0-pro", vec![Message::user("ask")]),
        )
        .await
        .unwrap();
    let non_stream_id = resp
        .message
        .content
        .iter()
        .find_map(|c| match c {
            ContentPart::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("non-streamed response has a ToolCall part");

    assert_eq!(
        streamed_id, non_stream_id,
        "streaming and non-streaming paths must produce the same id for the same (name, args)"
    );
    assert!(
        streamed_id.starts_with("vertex_call_"),
        "id must keep the `vertex_call_` prefix; got: {streamed_id}"
    );
}

#[tokio::test]
async fn stream_distinct_tool_calls_get_distinct_ids() {
    // Two function_call deltas in one stream with distinct
    // (name, args) pairs must produce distinct ids — proves the
    // hash isn't collapsing different calls into the same id.
    let server = MockServer::start().await;

    let chunks = [
        r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"city":"Tokyo"}}}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"city":"Paris"}}}]}}]}"#,
        r#"{"candidates":[{"finishReason":"STOP"}]}"#,
    ];
    let mut sse_body = String::new();
    for c in &chunks {
        sse_body.push_str("data: ");
        sse_body.push_str(c);
        sse_body.push_str("\n\n");
    }
    Mock::given(method("POST"))
        .and(path(
            "/v1/projects/my-proj/locations/us-central1/publishers/google/models/gemini-2.0-pro:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("Content-Type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let p = build_provider(&server.uri());
    let deltas = collect_stream_tool_calls(&p).await;
    assert_eq!(deltas.len(), 2, "expected two streamed tool-call deltas");
    let id_a = deltas[0].id.clone().unwrap();
    let id_b = deltas[1].id.clone().unwrap();
    assert_ne!(
        id_a, id_b,
        "distinct (name, args) must produce distinct ids; got duplicate {id_a}"
    );
    // Sanity: the within-stream `index` field still increments per
    // delta for chunk reassembly. This is the contract `id` no
    // longer doubles for after Phase 48.
    assert_eq!(deltas[0].index, 0);
    assert_eq!(deltas[1].index, 1);
}

#[tokio::test]
async fn rate_limit_maps_to_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_string("quota"))
        .mount(&server)
        .await;

    let p = build_provider(&server.uri());
    let err = p
        .chat(
            &Principal::anonymous(),
            ChatRequest::new("gemini-2.0-pro", vec![Message::user("hi")]),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, tako_core::TakoError::RateLimited(_)));
}
