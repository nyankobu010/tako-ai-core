//! Policy enforcement end-to-end: OPA bundle + SingleAgent + audit log.
//!
//! Spec §18 Phase 2 acceptance: "OPA bundle blocks a forbidden tool call
//! with a recorded audit log."
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::json;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, ContentPart, FinishReason, LlmProvider,
    Message, PolicyContext, PolicyDecision, PolicyEngine, PolicyStage, Principal, Role, TakoError,
    Tool, ToolSchema, Usage,
};
use tako_governance::{AuditLog, OpaBundle};
use tako_mcp::ToolRegistry;
use tako_orchestrator::{OrchInput, Orchestrator, SingleAgent};

const POLICY: &str = r#"
package tako.pre_tool

default decision := {"decision": "allow"}

decision := {"decision": "deny", "reason": "shell.exec requires admin role"} if {
    "shell.exec" in input.tools
    not "admin" in input.principal.roles
}
"#;

#[derive(Debug)]
struct FakeProvider {
    id: String,
    capabilities: Capabilities,
    responses: tokio::sync::Mutex<VecDeque<ChatResponse>>,
    calls: AtomicUsize,
}

impl FakeProvider {
    fn new(id: &str, responses: Vec<ChatResponse>) -> Self {
        Self {
            id: id.into(),
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
            .ok_or_else(|| TakoError::Invalid("FakeProvider: out of responses".into()))
    }
    async fn stream(
        &self,
        _p: &Principal,
        _r: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        Err(TakoError::Invalid("not implemented".into()))
    }
}

#[derive(Debug)]
struct ShellExecTool {
    schema: ToolSchema,
}

impl ShellExecTool {
    fn new() -> Self {
        Self {
            schema: ToolSchema {
                name: "shell.exec".into(),
                description: "Execute a shell command.".into(),
                input_schema: json!({"type": "object"}),
                annotations: None,
            },
        }
    }
}

#[async_trait]
impl Tool for ShellExecTool {
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
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, TakoError> {
        // If we get here, the policy let the call through.
        Ok(json!({"status": "ran"}))
    }
}

/// Wraps an OpaBundle so it can record every decision to an AuditLog.
struct AuditingPolicy {
    inner: OpaBundle,
    audit: AuditLog,
}

#[async_trait]
impl PolicyEngine for AuditingPolicy {
    async fn evaluate(
        &self,
        principal: &Principal,
        ctx: PolicyContext,
    ) -> Result<PolicyDecision, TakoError> {
        let decision = self.inner.evaluate(principal, ctx.clone()).await?;
        self.audit.record(principal, &ctx, &decision).await;
        Ok(decision)
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
        usage: Usage::default(),
        raw: Default::default(),
    }
}

#[tokio::test]
async fn opa_blocks_forbidden_tool_with_audit_record() {
    let provider = Arc::new(FakeProvider::new(
        "fake:m1",
        vec![assistant_tool_call(
            "call_1",
            "shell.exec",
            json!({"cmd": "ls"}),
        )],
    ));
    let registry = Arc::new(ToolRegistry::new());
    registry
        .register_native(Arc::new(ShellExecTool::new()))
        .await;

    let bundle = OpaBundle::from_string("pre_tool.rego", POLICY);
    let (audit, buf) = AuditLog::in_memory();
    let policy = Arc::new(AuditingPolicy {
        inner: bundle,
        audit,
    });

    let agent = SingleAgent::builder()
        .provider(provider.clone())
        .tools(registry)
        .max_steps(2)
        .policy(policy)
        .build()
        .unwrap();

    // Non-admin user → policy must block the tool call.
    let user = Principal {
        tenant_id: "acme".into(),
        user_id: "bob".into(),
        roles: vec!["user".into()],
        trace_id: None,
        metadata: Default::default(),
    };

    let err = agent
        .run(&user, OrchInput::from_user("call shell.exec"))
        .await
        .unwrap_err();
    let TakoError::PolicyDenied(reason) = err else {
        panic!("expected PolicyDenied, got {err:?}");
    };
    assert!(reason.contains("shell.exec"));
    assert!(reason.contains("admin"));

    // Audit log captured the decision.
    let g = buf.lock().unwrap();
    let log = std::str::from_utf8(&g).unwrap();
    assert!(log.contains("\"stage\":\"pre_tool\""));
    assert!(log.contains("deny"));
    assert!(log.contains("shell.exec"));
}

#[tokio::test]
async fn opa_allows_admin_through() {
    let provider = Arc::new(FakeProvider::new(
        "fake:m1",
        vec![
            assistant_tool_call("call_1", "shell.exec", json!({"cmd": "ls"})),
            ChatResponse {
                message: Message::assistant("done"),
                finish_reason: FinishReason::Stop,
                usage: Usage::default(),
                raw: Default::default(),
            },
        ],
    ));
    let registry = Arc::new(ToolRegistry::new());
    registry
        .register_native(Arc::new(ShellExecTool::new()))
        .await;

    let bundle = OpaBundle::from_string("pre_tool.rego", POLICY);
    let agent = SingleAgent::builder()
        .provider(provider.clone())
        .tools(registry)
        .max_steps(3)
        .policy(Arc::new(bundle))
        .build()
        .unwrap();

    let admin = Principal {
        tenant_id: "acme".into(),
        user_id: "alice".into(),
        roles: vec!["admin".into()],
        trace_id: None,
        metadata: Default::default(),
    };

    let result = agent
        .run(&admin, OrchInput::from_user("run command"))
        .await
        .unwrap();
    assert_eq!(result.text, "done");
    let _ = PolicyStage::PostChat;
}
