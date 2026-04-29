//! `Trinity` orchestrator: a Router-driven multi-role agent.
//!
//! Generalisation of arXiv:2512.04695 (Sakana AI's *Trinity*). Each turn,
//! a [`Router`] picks one role from a pool of `(role_name → provider)`
//! pairs (e.g. `Thinker`, `Worker`, `Verifier`). The selected provider
//! handles that turn's chat call; tool calls are looped just like
//! [`crate::SingleAgent`].
//!
//! Trinity is most useful when paired with a learned router (the
//! [`crate::OnnxRouter`] from Phase 3) so the role + model selection
//! is data-driven, not coordinator-driven (Conductor) or static
//! (SingleAgent).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tako_core::{
    ChatRequest, ContentPart, FinishReason, LlmProvider, Message, PolicyContext, PolicyDecision,
    PolicyEngine, PolicyStage, Principal, Role, Router, TakoError, Usage,
};
use tako_mcp::ToolRegistry;
use tracing::{Instrument, info_span};

use crate::single::{ChatDefaults, hash_messages_pub, hash_value_pub};
use crate::types::{OrchEvent, OrchInput, OrchOutput};
use crate::{Orchestrator, OrchestratorKind};

const DEFAULT_MAX_STEPS: u32 = 8;

/// Trinity orchestrator: per-turn role + model selection via a Router.
pub struct Trinity {
    roles: HashMap<String, Arc<dyn LlmProvider>>,
    /// Stable ordering of role names — matches the candidate vector
    /// passed to the router so positional indexing is deterministic.
    role_order: Vec<String>,
    router: Arc<dyn Router>,
    tools: Arc<ToolRegistry>,
    max_steps: u32,
    defaults: ChatDefaults,
    policy: Option<Arc<dyn PolicyEngine>>,
}

impl std::fmt::Debug for Trinity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trinity")
            .field("roles", &self.role_order)
            .field("max_steps", &self.max_steps)
            .finish()
    }
}

impl Trinity {
    pub fn builder() -> TrinityBuilder {
        TrinityBuilder::default()
    }
}

#[derive(Default)]
pub struct TrinityBuilder {
    roles: HashMap<String, Arc<dyn LlmProvider>>,
    role_order: Vec<String>,
    router: Option<Arc<dyn Router>>,
    tools: Option<Arc<ToolRegistry>>,
    max_steps: Option<u32>,
    defaults: ChatDefaults,
    policy: Option<Arc<dyn PolicyEngine>>,
}

impl std::fmt::Debug for TrinityBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrinityBuilder")
            .field("roles", &self.role_order)
            .field("max_steps", &self.max_steps)
            .finish_non_exhaustive()
    }
}

impl TrinityBuilder {
    pub fn role(mut self, name: impl Into<String>, p: Arc<dyn LlmProvider>) -> Self {
        let name = name.into();
        if !self.roles.contains_key(&name) {
            self.role_order.push(name.clone());
        }
        self.roles.insert(name, p);
        self
    }

    pub fn router(mut self, r: Arc<dyn Router>) -> Self {
        self.router = Some(r);
        self
    }

    pub fn tools(mut self, t: Arc<ToolRegistry>) -> Self {
        self.tools = Some(t);
        self
    }

    pub fn max_steps(mut self, n: u32) -> Self {
        self.max_steps = Some(n.max(1));
        self
    }

    pub fn temperature(mut self, t: f32) -> Self {
        self.defaults.temperature = Some(t);
        self
    }

    pub fn max_tokens(mut self, n: u32) -> Self {
        self.defaults.max_tokens = Some(n);
        self
    }

    pub fn policy(mut self, p: Arc<dyn PolicyEngine>) -> Self {
        self.policy = Some(p);
        self
    }

    pub fn build(self) -> Result<Trinity, TakoError> {
        if self.roles.is_empty() {
            return Err(TakoError::Invalid(
                "TrinityBuilder: at least one role is required".into(),
            ));
        }
        let router = self
            .router
            .ok_or_else(|| TakoError::Invalid("TrinityBuilder: router is required".into()))?;
        Ok(Trinity {
            roles: self.roles,
            role_order: self.role_order,
            router,
            tools: self.tools.unwrap_or_else(|| Arc::new(ToolRegistry::new())),
            max_steps: self.max_steps.unwrap_or(DEFAULT_MAX_STEPS),
            defaults: self.defaults,
            policy: self.policy,
        })
    }
}

impl Trinity {
    /// Provider-id list passed to the router as the candidate pool. The
    /// strings are `"role_name|provider_id"` so the router can use either
    /// the role label or the underlying provider id when emitting
    /// `RoutingDecision::provider_id`.
    fn candidate_ids(&self) -> Vec<String> {
        self.role_order
            .iter()
            .map(|r| {
                let pid = self
                    .roles
                    .get(r)
                    .map(|p| p.id().to_string())
                    .unwrap_or_default();
                format!("{r}|{pid}")
            })
            .collect()
    }

    fn pick_provider(&self, decision_id: &str) -> Option<(String, Arc<dyn LlmProvider>)> {
        // Decision may be either a `role|pid` candidate string, a bare
        // role name, or a bare provider id. Try in that order.
        if let Some((role, _pid)) = decision_id.split_once('|') {
            if let Some(p) = self.roles.get(role) {
                return Some((role.to_string(), p.clone()));
            }
        }
        if let Some(p) = self.roles.get(decision_id) {
            return Some((decision_id.to_string(), p.clone()));
        }
        for (role, p) in &self.roles {
            if p.id() == decision_id {
                return Some((role.clone(), p.clone()));
            }
        }
        None
    }
}

#[async_trait]
impl Orchestrator for Trinity {
    fn kind(&self) -> OrchestratorKind {
        OrchestratorKind::Trinity
    }

    async fn run(&self, principal: &Principal, input: OrchInput) -> Result<OrchOutput, TakoError> {
        let span = info_span!(
            "tako.orchestrator.run",
            "tako.orchestrator.kind" = "trinity",
            "tako.principal.tenant_id" = %principal.tenant_id,
            "tako.principal.user_id" = %principal.user_id,
        );
        async move {
            let mut messages: Vec<Message> = Vec::new();
            if let Some(sys) = input.system.clone() {
                messages.push(Message::system(sys));
            }
            messages.extend(input.messages);

            let mut total_usage = Usage::default();
            let mut steps = 0_u32;
            let mut final_message: Option<Message> = None;
            let candidates = self.candidate_ids();

            for step in 0..self.max_steps {
                let req_for_router = ChatRequest::new("router", messages.clone());
                let decision = self
                    .router
                    .route(principal, &req_for_router, &candidates)
                    .await?;
                let (role, provider) =
                    self.pick_provider(&decision.provider_id).ok_or_else(|| {
                        TakoError::Invalid(format!(
                            "Trinity: router chose unknown provider id `{}`",
                            decision.provider_id
                        ))
                    })?;

                let model = provider.id().split(':').nth(1).unwrap_or("").to_string();
                let req = ChatRequest {
                    model: model.clone(),
                    messages: messages.clone(),
                    tools: self.tools.schemas().await,
                    temperature: self.defaults.temperature,
                    max_tokens: self.defaults.max_tokens,
                    stop: Vec::new(),
                    stream: false,
                    metadata: Default::default(),
                };
                let resp = {
                    let span = info_span!(
                        "tako.provider.chat",
                        "tako.provider.id" = %provider.id(),
                        "tako.provider.model" = %model,
                        "tako.orchestrator.step" = step,
                        "tako.orchestrator.role" = %role,
                    );
                    provider.chat(principal, req).instrument(span).await?
                };
                steps += 1;
                total_usage.input_tokens = total_usage
                    .input_tokens
                    .saturating_add(resp.usage.input_tokens);
                total_usage.output_tokens = total_usage
                    .output_tokens
                    .saturating_add(resp.usage.output_tokens);

                let assistant = resp.message.clone();
                messages.push(assistant.clone());

                let tool_calls: Vec<(String, String, serde_json::Value)> = assistant
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ContentPart::ToolCall { id, name, args } => {
                            Some((id.clone(), name.clone(), args.clone()))
                        }
                        _ => None,
                    })
                    .collect();

                if tool_calls.is_empty()
                    || matches!(
                        resp.finish_reason,
                        FinishReason::Stop | FinishReason::Length
                    ) && tool_calls.is_empty()
                {
                    final_message = Some(assistant);
                    break;
                }

                let mut tool_results: Vec<ContentPart> = Vec::with_capacity(tool_calls.len());
                for (id, name, args) in tool_calls {
                    if let Some(engine) = &self.policy {
                        let ctx = PolicyContext {
                            stage: PolicyStage::PreTool,
                            model: model.clone(),
                            messages_hash: hash_messages_pub(&messages),
                            tools: vec![name.clone()],
                            tool_args_hash: Some(hash_value_pub(&args)),
                            response_hash: None,
                        };
                        match engine.evaluate(principal, ctx).await? {
                            PolicyDecision::Deny { reason } => {
                                return Err(TakoError::PolicyDenied(format!(
                                    "tool `{name}`: {reason}"
                                )));
                            }
                            PolicyDecision::RequireApproval { reason } => {
                                return Err(TakoError::PolicyDenied(format!(
                                    "tool `{name}` requires approval: {reason}"
                                )));
                            }
                            _ => {}
                        }
                    }
                    let result = match self.tools.invoke(principal, &name, args).await {
                        Ok(v) => ContentPart::ToolResult {
                            id,
                            result: v,
                            is_error: false,
                        },
                        Err(e) => ContentPart::ToolResult {
                            id,
                            result: serde_json::json!({ "error": e.to_string() }),
                            is_error: true,
                        },
                    };
                    tool_results.push(result);
                }
                messages.push(Message {
                    role: Role::User,
                    content: tool_results,
                });

                if step + 1 == self.max_steps {
                    final_message = Some(assistant);
                }
            }

            let final_message = final_message.unwrap_or_else(|| Message::assistant(""));
            let text = final_message
                .content
                .iter()
                .filter_map(ContentPart::as_text)
                .collect::<Vec<_>>()
                .join("");
            Ok(OrchOutput {
                text,
                message: final_message,
                usage: total_usage,
                steps,
            })
        }
        .instrument(span)
        .await
    }

    async fn stream(
        &self,
        _principal: &Principal,
        _input: OrchInput,
    ) -> BoxStream<'static, Result<OrchEvent, TakoError>> {
        // Delegated to `run`: emit one StepStart + one AssistantText with
        // the full final text + Final. Trinity doesn't yet forward
        // provider-level deltas because the routed provider varies per
        // turn; that's a Phase 4 polish item.
        Box::pin(futures::stream::once(async move {
            Err(TakoError::Invalid(
                "Trinity streaming is Phase 4; use `run` for now".into(),
            ))
        }))
    }
}
