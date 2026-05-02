//! `SingleAgent` orchestrator tests using a hand-rolled `FakeProvider`
//! and a built-in echo tool.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::json;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, ContentPart, FinishReason, LlmProvider,
    Message, Principal, Role, TakoError, Tool, ToolSchema, Usage,
};
use tako_mcp::ToolRegistry;
use tako_orchestrator::{OrchEvent, OrchInput, Orchestrator, SingleAgent};

#[derive(Debug)]
struct FakeProvider {
    id: String,
    capabilities: Capabilities,
    /// Scripted responses; each call pops one off the front.
    responses: tokio::sync::Mutex<std::collections::VecDeque<ChatResponse>>,
    calls: AtomicUsize,
}

impl FakeProvider {
    fn new(id: &str, responses: Vec<ChatResponse>) -> Self {
        Self {
            id: id.to_string(),
            capabilities: Capabilities::default(),
            responses: tokio::sync::Mutex::new(responses.into()),
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
            .ok_or_else(|| TakoError::Invalid("FakeProvider: no scripted response left".into()))
    }
    async fn stream(
        &self,
        _p: &Principal,
        _r: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        Err(TakoError::Invalid("not implemented".into()))
    }
}

/// Echo tool: returns its input verbatim.
#[derive(Debug)]
struct EchoTool {
    schema: ToolSchema,
}

impl EchoTool {
    fn new() -> Self {
        Self {
            schema: ToolSchema {
                name: "echo".into(),
                description: "Echo input verbatim.".into(),
                input_schema: json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
                annotations: None,
            },
        }
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.schema.name
    }
    fn description(&self) -> &str {
        &self.schema.description
    }
    fn schema(&self) -> &ToolSchema {
        &self.schema
    }
    async fn invoke(
        &self,
        _principal: &Principal,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, TakoError> {
        Ok(args)
    }
}

fn assistant_text(text: &str) -> ChatResponse {
    ChatResponse {
        message: Message::assistant(text),
        finish_reason: FinishReason::Stop,
        usage: Usage {
            input_tokens: 5,
            output_tokens: 3,
        },
        raw: Default::default(),
    }
}

fn assistant_tool_call(id: &str, name: &str, args: serde_json::Value) -> ChatResponse {
    ChatResponse {
        message: Message {
            role: Role::Assistant,
            content: vec![ContentPart::ToolCall {
                id: id.into(),
                name: name.into(),
                args,
            }],
        },
        finish_reason: FinishReason::ToolCalls,
        usage: Usage {
            input_tokens: 4,
            output_tokens: 8,
        },
        raw: Default::default(),
    }
}

#[tokio::test]
async fn single_agent_no_tools_returns_text() {
    let provider = Arc::new(FakeProvider::new(
        "fake:m1",
        vec![assistant_text("hello, world")],
    ));
    let agent = SingleAgent::builder()
        .provider(provider.clone())
        .max_steps(3)
        .build()
        .unwrap();
    let result = agent
        .run(&Principal::anonymous(), OrchInput::from_user("hi"))
        .await
        .unwrap();
    assert_eq!(result.text, "hello, world");
    assert_eq!(result.steps, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn single_agent_loops_through_tool_call() {
    let provider = Arc::new(FakeProvider::new(
        "fake:m1",
        vec![
            assistant_tool_call("call_1", "echo", json!({"text":"ping"})),
            assistant_text("got it"),
        ],
    ));

    let registry = Arc::new(ToolRegistry::new());
    registry.register_native(Arc::new(EchoTool::new())).await;

    let agent = SingleAgent::builder()
        .provider(provider.clone())
        .tools(registry)
        .max_steps(3)
        .build()
        .unwrap();

    let result = agent
        .run(&Principal::anonymous(), OrchInput::from_user("call echo"))
        .await
        .unwrap();
    assert_eq!(result.text, "got it");
    assert_eq!(result.steps, 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn single_agent_stream_emits_events_in_order() {
    let provider = Arc::new(FakeProvider::new(
        "fake:m1",
        vec![
            assistant_tool_call("call_1", "echo", json!({"text":"ping"})),
            assistant_text("got it"),
        ],
    ));
    let registry = Arc::new(ToolRegistry::new());
    registry.register_native(Arc::new(EchoTool::new())).await;

    let agent = SingleAgent::builder()
        .provider(provider.clone())
        .tools(registry)
        .max_steps(3)
        .build()
        .unwrap();

    let mut stream = agent
        .stream(&Principal::anonymous(), OrchInput::from_user("call echo"))
        .await;

    let mut kinds: Vec<&'static str> = Vec::new();
    let mut final_text: Option<String> = None;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            OrchEvent::StepStart { .. } => kinds.push("step_start"),
            OrchEvent::AssistantText { delta, .. } if !delta.is_empty() => {
                kinds.push("assistant_text");
            }
            OrchEvent::AssistantText { .. } => {}
            OrchEvent::ToolCallStart { .. } => kinds.push("tool_call_start"),
            OrchEvent::ToolCallResult { is_error, .. } => {
                assert!(!is_error);
                kinds.push("tool_call_result");
            }
            OrchEvent::Final { output } => {
                kinds.push("final");
                final_text = Some(output.text.clone());
                assert_eq!(output.steps, 2);
            }
            _ => {}
        }
    }

    assert_eq!(
        kinds,
        vec![
            "step_start",
            "tool_call_start",
            "tool_call_result",
            "step_start",
            "assistant_text",
            "final",
        ]
    );
    assert_eq!(final_text.as_deref(), Some("got it"));
}

#[tokio::test]
async fn single_agent_with_router_picks_candidate() {
    use tako_orchestrator::RegexRouter;
    let primary = Arc::new(FakeProvider::new(
        "fake:fb",
        vec![assistant_text("FALLBACK")],
    ));
    // Candidate 0 receives the math-flagged prompt because RegexRouter::default()
    // maps math features to candidate-index 1; with [primary, math_cand] the
    // router's index-1 corresponds to math_cand.
    // Default RegexRouter rule maps:
    //   - code keywords → idx 0
    //   - math keywords → idx 1
    //   - default       → idx 2
    // SingleAgent passes [primary, ...candidates] as the candidate list, so
    // we need a primary + one candidate; default_idx=2 will exceed the list
    // and clamp to len-1 (=1, the candidate). For a math prompt, idx=1 hits
    // the candidate too. So candidate is selected for both math and chitchat;
    // primary only for code.
    let candidate = Arc::new(FakeProvider::new(
        "fake:cand",
        vec![assistant_text("FROM CAND")],
    ));
    let agent = SingleAgent::builder()
        .provider(primary.clone())
        .candidate(candidate.clone())
        .router(Arc::new(RegexRouter::default()))
        .max_steps(1)
        .build()
        .unwrap();
    let result = agent
        .run(&Principal::anonymous(), OrchInput::from_user("Solve 2+2"))
        .await
        .unwrap();
    assert_eq!(result.text, "FROM CAND");
    assert_eq!(candidate.calls.load(Ordering::SeqCst), 1);
    assert_eq!(primary.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn single_agent_respects_max_steps() {
    // The provider keeps emitting tool calls, but max_steps caps the loop.
    let provider = Arc::new(FakeProvider::new(
        "fake:m1",
        vec![
            assistant_tool_call("c1", "echo", json!({"text":"a"})),
            assistant_tool_call("c2", "echo", json!({"text":"b"})),
        ],
    ));
    let registry = Arc::new(ToolRegistry::new());
    registry.register_native(Arc::new(EchoTool::new())).await;

    let agent = SingleAgent::builder()
        .provider(provider.clone())
        .tools(registry)
        .max_steps(2)
        .build()
        .unwrap();

    let result = agent
        .run(&Principal::anonymous(), OrchInput::from_user("loop"))
        .await
        .unwrap();
    assert_eq!(result.steps, 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// Budget wiring (Phase 5.C).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn budget_record_accumulates_usage_across_steps() {
    use tako_core::Budget;
    use tako_runtime::{BudgetBackend, BudgetTracker, InMemoryBudgetBackend};

    let backend = Arc::new(InMemoryBudgetBackend::new());
    let tracker = Arc::new(BudgetTracker::new(
        Arc::clone(&backend) as Arc<dyn BudgetBackend>,
        Budget::default(),
    ));

    let provider = Arc::new(FakeProvider::new(
        "fake:m1",
        vec![
            assistant_tool_call("c1", "echo", json!({"text":"a"})),
            assistant_text("done"),
        ],
    ));
    let registry = Arc::new(ToolRegistry::new());
    registry.register_native(Arc::new(EchoTool::new())).await;

    let agent = SingleAgent::builder()
        .provider(provider.clone())
        .tools(registry)
        .max_steps(3)
        .budget(Arc::clone(&tracker))
        .build()
        .unwrap();

    let principal = Principal {
        tenant_id: "tenant-a".into(),
        user_id: "user-1".into(),
        roles: vec![],
        trace_id: None,
        metadata: Default::default(),
    };
    agent
        .run(&principal, OrchInput::from_user("hi"))
        .await
        .unwrap();

    // Two provider calls — usage from each ChatResponse should land in
    // the backend keyed by tenant_id.
    let usage = backend.current_usage("tenant-a").await.unwrap();
    // FakeProvider's tool-call response has 4+8=12 tokens; the text
    // response 5+3=8. Total: 20.
    assert_eq!(usage.tokens_today, 20);
}

#[tokio::test]
async fn budget_pre_check_short_circuits_on_request_cap() {
    use std::collections::BTreeMap;
    use tako_core::Budget;
    use tako_runtime::{BudgetBackend, BudgetTracker, InMemoryBudgetBackend};

    // FakeProvider's estimate_cost_usd defaults to 0.0, so per-request
    // dollar cap won't trip. Use the per-request *token* cap instead —
    // the orchestrator passes `req.max_tokens` as the pre-check token
    // estimate, so a `max_tokens=64` request with cap 16 must fail.
    let backend = Arc::new(InMemoryBudgetBackend::new());
    let tracker = Arc::new(BudgetTracker::new(
        Arc::clone(&backend) as Arc<dyn BudgetBackend>,
        Budget {
            max_usd_per_request: None,
            max_tokens_per_request: Some(16),
            max_usd_per_day: None,
            max_usd_per_tenant_per_day: BTreeMap::new(),
        },
    ));

    let provider = Arc::new(FakeProvider::new(
        "fake:m1",
        vec![assistant_text("never reached")],
    ));
    let agent = SingleAgent::builder()
        .provider(provider.clone())
        .max_steps(2)
        .max_tokens(64)
        .budget(Arc::clone(&tracker))
        .build()
        .unwrap();

    let result = agent
        .run(&Principal::anonymous(), OrchInput::from_user("hi"))
        .await;
    let err = result.unwrap_err();
    assert!(
        matches!(err, TakoError::BudgetExhausted(_)),
        "expected BudgetExhausted, got {err:?}"
    );
    // Provider must NOT have been called.
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}
