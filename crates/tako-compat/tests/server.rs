//! End-to-end test of the compat server using a real HTTP client.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::json;
use tako_compat::{ServeConfig, StaticTokens, serve_openai};
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, FinishReason, LlmProvider, Message,
    Principal, TakoError, Usage,
};
use tako_orchestrator::SingleAgent;

#[derive(Debug)]
struct FakeProvider {
    id: String,
    capabilities: Capabilities,
    responses: tokio::sync::Mutex<VecDeque<ChatResponse>>,
    calls: AtomicUsize,
}

impl FakeProvider {
    fn with_canned(id: &str, text: &str) -> Self {
        Self {
            id: id.into(),
            capabilities: Capabilities::default(),
            responses: tokio::sync::Mutex::new(
                vec![ChatResponse {
                    message: Message::assistant(text),
                    finish_reason: FinishReason::Stop,
                    usage: Usage {
                        input_tokens: 5,
                        output_tokens: 3,
                    },
                    raw: Default::default(),
                }]
                .into(),
            ),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for FakeProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    async fn chat(
        &self,
        _principal: &Principal,
        _req: ChatRequest,
    ) -> Result<ChatResponse, TakoError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| TakoError::Invalid("FakeProvider: out".into()))
    }
    async fn stream(
        &self,
        _p: &Principal,
        _r: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        Err(TakoError::Invalid("not implemented".into()))
    }
}

async fn boot_server() -> (std::net::SocketAddr, Arc<FakeProvider>) {
    let provider = Arc::new(FakeProvider::with_canned("fake:m", "hello from compat"));
    let agent = Arc::new(
        SingleAgent::builder()
            .provider(provider.clone())
            .max_steps(2)
            .build()
            .unwrap(),
    );

    let auth = Arc::new(StaticTokens::new().with("test-token", Principal::new("acme", "alice")));

    let (addr, _handle) = serve_openai(
        agent,
        auth,
        ServeConfig {
            host: "127.0.0.1".into(),
            port: 0,
            models: vec!["fake:m".into(), "tako-default".into()],
        },
    )
    .await
    .unwrap();
    (addr, provider)
}

#[tokio::test]
async fn chat_completions_round_trip() {
    let (addr, provider) = boot_server().await;
    let client = reqwest::Client::new();
    let body = json!({
        "model": "tako-default",
        "messages": [{"role": "user", "content": "hi"}],
    });
    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Authorization", "Bearer test-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["object"], "chat.completion");
    assert_eq!(json["choices"][0]["message"]["role"], "assistant");
    assert_eq!(
        json["choices"][0]["message"]["content"],
        "hello from compat"
    );
    assert_eq!(json["choices"][0]["finish_reason"], "stop");
    assert_eq!(json["usage"]["prompt_tokens"], 5);
    assert_eq!(json["usage"]["completion_tokens"], 3);
    assert_eq!(json["usage"]["total_tokens"], 8);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn missing_bearer_returns_401() {
    let (addr, _) = boot_server().await;
    let client = reqwest::Client::new();
    let body = json!({"model": "tako-default", "messages": [{"role": "user", "content": "x"}]});
    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn unknown_token_returns_401() {
    let (addr, _) = boot_server().await;
    let client = reqwest::Client::new();
    let body = json!({"model": "tako-default", "messages": [{"role": "user", "content": "x"}]});
    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Authorization", "Bearer wrong")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn stream_request_returns_sse_chunks_and_done() {
    let (addr, _) = boot_server().await;
    let client = reqwest::Client::new();
    let body = json!({
        "model": "tako-default",
        "messages": [{"role": "user", "content": "x"}],
        "stream": true,
    });
    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Authorization", "Bearer test-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.starts_with("text/event-stream"),
        "expected text/event-stream, got {ct}"
    );
    let bytes = resp.bytes().await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    // The stream must include at least one chat.completion.chunk and end
    // with a `data: [DONE]` line — the OpenAI SDK's parser bails
    // otherwise.
    assert!(text.contains("chat.completion.chunk"), "saw: {text}");
    assert!(text.contains("data: [DONE]"), "saw: {text}");
}

/// Phase 9.C — a stub Orchestrator that emits a fixed event sequence
/// for the SSE wire-format test below. Lets the test assert the
/// `event: tako.*` named-extension framing without standing up a
/// full AbMcts pipeline.
#[derive(Debug)]
struct ScriptedOrchestrator {
    events: tokio::sync::Mutex<Vec<tako_orchestrator::OrchEvent>>,
}

#[async_trait]
impl tako_orchestrator::Orchestrator for ScriptedOrchestrator {
    fn kind(&self) -> tako_orchestrator::OrchestratorKind {
        // Reuse SingleAgent — the kind is informational only here.
        tako_orchestrator::OrchestratorKind::SingleAgent
    }
    async fn run(
        &self,
        _principal: &Principal,
        _input: tako_orchestrator::OrchInput,
    ) -> Result<tako_orchestrator::OrchOutput, TakoError> {
        Err(TakoError::Invalid("scripted: run not used".into()))
    }
    async fn stream(
        &self,
        _principal: &Principal,
        _input: tako_orchestrator::OrchInput,
    ) -> BoxStream<'static, Result<tako_orchestrator::OrchEvent, TakoError>> {
        let evs = std::mem::take(&mut *self.events.lock().await);
        let s = futures::stream::iter(evs.into_iter().map(Ok::<_, TakoError>));
        futures::StreamExt::boxed(s)
    }
}

#[tokio::test]
async fn stream_emits_named_tako_extension_for_verifier_score() {
    use tako_orchestrator::{OrchEvent, OrchOutput};
    let final_event = OrchEvent::Final {
        output: Box::new(OrchOutput {
            text: "done".into(),
            message: Message::assistant("done"),
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
            steps: 1,
        }),
    };
    let scripted = Arc::new(ScriptedOrchestrator {
        events: tokio::sync::Mutex::new(vec![
            OrchEvent::StepStart { step: 0 },
            OrchEvent::VerifierScore {
                step: 0,
                branch: 1,
                score: 0.5,
            },
            OrchEvent::AssistantText {
                step: 0,
                delta: "hello".into(),
            },
            final_event,
        ]),
    });
    let auth = Arc::new(StaticTokens::new().with("test-token", Principal::new("acme", "alice")));
    let (addr, _handle) = serve_openai(
        scripted,
        auth,
        ServeConfig {
            host: "127.0.0.1".into(),
            port: 0,
            models: vec!["fake:m".into(), "tako-default".into()],
        },
    )
    .await
    .unwrap();

    let client = reqwest::Client::new();
    let body = json!({
        "model": "tako-default",
        "messages": [{"role": "user", "content": "x"}],
        "stream": true,
    });
    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Authorization", "Bearer test-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    // The named extension frame must appear (event: line + data: line),
    // and must precede the OpenAI assistant-text data: chunk that
    // follows the same logical event boundary.
    let ext_idx = text
        .find("event: tako.verifier_score")
        .unwrap_or_else(|| panic!("missing tako.verifier_score frame; saw: {text}"));
    let assist_idx = text
        .find("\"content\":\"hello\"")
        .unwrap_or_else(|| panic!("missing assistant-text data: chunk; saw: {text}"));
    assert!(
        ext_idx < assist_idx,
        "tako.verifier_score must precede the related assistant-text frame; saw: {text}",
    );
    assert!(text.contains("\"branch\":1"), "saw: {text}");
    assert!(text.contains("data: [DONE]"), "saw: {text}");
}

#[tokio::test]
async fn stream_emits_tool_call_lifecycle_extensions() {
    // Phase 10.B — `ToolCallStart` and `ToolCallResult` flow through
    // the SSE pipeline with both their existing OpenAI mappings (where
    // applicable) AND named `tako.tool_call_*` extension frames so
    // tako-aware consumers gain a typed handle on the lifecycle and
    // (crucially) on tool *results*, which previously had no
    // observable representation in the OpenAI mapping.
    use tako_orchestrator::{OrchEvent, OrchOutput};
    let final_event = OrchEvent::Final {
        output: Box::new(OrchOutput {
            text: "done".into(),
            message: Message::assistant("done"),
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
            steps: 1,
        }),
    };
    let scripted = Arc::new(ScriptedOrchestrator {
        events: tokio::sync::Mutex::new(vec![
            OrchEvent::StepStart { step: 0 },
            OrchEvent::ToolCallStart {
                step: 0,
                name: "search".into(),
                id: "tc-abc".into(),
            },
            OrchEvent::ToolCallResult {
                step: 0,
                id: "tc-abc".into(),
                result: serde_json::json!({"ok": true, "rows": 3}),
                is_error: false,
            },
            OrchEvent::AssistantText {
                step: 0,
                delta: "result handled".into(),
            },
            final_event,
        ]),
    });
    let auth = Arc::new(StaticTokens::new().with("test-token", Principal::new("acme", "alice")));
    let (addr, _handle) = serve_openai(
        scripted,
        auth,
        ServeConfig {
            host: "127.0.0.1".into(),
            port: 0,
            models: vec!["fake:m".into(), "tako-default".into()],
        },
    )
    .await
    .unwrap();

    let client = reqwest::Client::new();
    let body = json!({
        "model": "tako-default",
        "messages": [{"role": "user", "content": "x"}],
        "stream": true,
    });
    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Authorization", "Bearer test-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    let text = String::from_utf8_lossy(&bytes);

    // Both lifecycle frames are present, with their JSON payloads
    // carrying the relevant tako-side fields.
    let start_idx = text
        .find("event: tako.tool_call_start")
        .unwrap_or_else(|| panic!("missing tako.tool_call_start frame; saw: {text}"));
    let result_idx = text
        .find("event: tako.tool_call_result")
        .unwrap_or_else(|| panic!("missing tako.tool_call_result frame; saw: {text}"));
    assert!(
        start_idx < result_idx,
        "tako.tool_call_start must precede tako.tool_call_result; saw: {text}",
    );

    // Result payload preserves the structured tool result and is_error.
    assert!(text.contains("\"id\":\"tc-abc\""), "saw: {text}");
    assert!(text.contains("\"name\":\"search\""), "saw: {text}");
    assert!(text.contains("\"is_error\":false"), "saw: {text}");
    assert!(text.contains("\"rows\":3"), "saw: {text}");

    // OpenAI mapping for ToolCallStart is unchanged — the `tool_calls`
    // delta frame is still present and follows after the named
    // extension.
    let oa_tool_idx = text
        .find("\"tool_calls\"")
        .unwrap_or_else(|| panic!("missing OpenAI tool_calls delta; saw: {text}"));
    assert!(
        start_idx < oa_tool_idx,
        "tako.tool_call_start must precede the OpenAI tool_calls delta; saw: {text}",
    );

    // The downstream assistant-text and [DONE] sentinel still emit
    // unchanged.
    assert!(
        text.contains("\"content\":\"result handled\""),
        "saw: {text}"
    );
    assert!(text.contains("data: [DONE]"), "saw: {text}");
}

#[tokio::test]
async fn list_models() {
    let (addr, _) = boot_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let ids: Vec<&str> = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"fake:m"));
    assert!(ids.contains(&"tako-default"));
}

#[tokio::test]
async fn healthz_and_readyz() {
    let (addr, _) = boot_server().await;
    let client = reqwest::Client::new();
    for path in ["/healthz", "/readyz"] {
        let resp = client
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
