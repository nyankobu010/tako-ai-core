//! `SingleAgent` orchestrator tests using a hand-rolled `FakeProvider`
//! and a built-in echo tool.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::json;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, ContentPart, FinishReason, LlmProvider, Message, Principal,
    Role, TakoError, Tool, ToolSchema, Usage,
};
use tako_mcp::ToolRegistry;
use tako_orchestrator::{Orchestrator, OrchInput, SingleAgent};

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
    async fn chat(&self, _principal: &Principal, _req: ChatRequest) -> Result<ChatResponse, TakoError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| TakoError::Invalid("FakeProvider: no scripted response left".into()))
    }
    async fn stream(&self, _p: &Principal, _r: ChatRequest) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
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
    async fn invoke(&self, _principal: &Principal, args: serde_json::Value) -> Result<serde_json::Value, TakoError> {
        Ok(args)
    }
}

fn assistant_text(text: &str) -> ChatResponse {
    ChatResponse {
        message: Message::assistant(text),
        finish_reason: FinishReason::Stop,
        usage: Usage { input_tokens: 5, output_tokens: 3 },
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
        usage: Usage { input_tokens: 4, output_tokens: 8 },
        raw: Default::default(),
    }
}

#[tokio::test]
async fn single_agent_no_tools_returns_text() {
    let provider = Arc::new(FakeProvider::new("fake:m1", vec![assistant_text("hello, world")]));
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
