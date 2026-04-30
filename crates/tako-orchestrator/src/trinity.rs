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
use futures::StreamExt;
use futures::stream::BoxStream;
use tako_core::{
    ChatChunk, ChatRequest, ContentPart, FinishReason, LlmProvider, Message, PolicyContext,
    PolicyDecision, PolicyEngine, PolicyStage, Principal, Role, Router, TakoError, ToolCallDelta,
    Usage, Verifier,
};
use tako_mcp::ToolRegistry;
use tako_runtime::BudgetTracker;
use tracing::{Instrument, info_span};

use crate::single::{ChatDefaults, assemble_tool_calls_pub, hash_messages_pub, hash_value_pub};
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
    /// Optional budget tracker consulted before each provider call
    /// (`pre_check`) and after each call (`record`). When `None`, no
    /// budget enforcement runs and the orchestrator behaves exactly as
    /// in v0.6.0.
    budget: Option<Arc<BudgetTracker>>,
    /// Phase 10.C — optional [`Verifier`]. When set, the streaming
    /// path emits an [`OrchEvent::VerifierScore`] after each role's
    /// assistant turn completes, with `branch` = the role's
    /// positional index in `role_order`. Without this, no
    /// `VerifierScore` events appear (Trinity behaves exactly as in
    /// v0.10.0).
    verifier: Option<Arc<dyn Verifier>>,
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
    budget: Option<Arc<BudgetTracker>>,
    verifier: Option<Arc<dyn Verifier>>,
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

    /// Attach a [`BudgetTracker`]. When set, the orchestrator calls
    /// `pre_check` before each provider invocation and `record` after,
    /// using [`tako_core::LlmProvider::estimate_cost_usd`] for both the
    /// pre-flight estimate and the post-call cost. `BudgetExhausted`
    /// short-circuits the run.
    pub fn budget(mut self, t: Arc<BudgetTracker>) -> Self {
        self.budget = Some(t);
        self
    }

    /// Phase 10.C — attach a [`Verifier`]. When set, the streaming
    /// path emits one [`OrchEvent::VerifierScore`] after each role's
    /// assistant turn completes, scoring `(input_prompt, assistant_text)`
    /// at synthesis-complete boundaries (never per-delta). `branch`
    /// is the role's positional index in `role_order` so consumers
    /// can attribute the score to the specific role/provider that
    /// produced the turn. Without this builder method, no
    /// `VerifierScore` events are emitted (Trinity behaves exactly
    /// as in v0.10.0).
    ///
    /// Per-step verifier calls compose with whatever verifier
    /// implementation you choose; for cost-controlled streaming
    /// guards, see `LlmJudgeGuard` and `RuleBasedGuard` in
    /// [`crate::self_caller`].
    pub fn verifier(mut self, v: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(v);
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
            budget: self.budget,
            verifier: self.verifier,
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
                let estimated_usd = provider.estimate_cost_usd(&req);
                if let Some(b) = self.budget.as_ref() {
                    let est_tokens = req.max_tokens.unwrap_or(0);
                    b.pre_check(principal, estimated_usd, est_tokens).await?;
                }
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
                if let Some(b) = self.budget.as_ref() {
                    b.record(principal, estimated_usd, resp.usage).await?;
                }
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
        principal: &Principal,
        input: OrchInput,
    ) -> BoxStream<'static, Result<OrchEvent, TakoError>> {
        let roles = self.roles.clone();
        let role_order = self.role_order.clone();
        let router = self.router.clone();
        let tools = self.tools.clone();
        let policy = self.policy.clone();
        let defaults = self.defaults.clone();
        let max_steps = self.max_steps;
        let principal = principal.clone();
        let budget = self.budget.clone();
        let verifier = self.verifier.clone();

        let s = async_stream::try_stream! {
            // Phase 10.C — derive a single prompt-text snapshot from the
            // user-side input, used as the verifier's `prompt` argument
            // for every per-step scoring call. Mirrors the AB-MCTS
            // `prompt_text` derivation so the two orchestrators feed
            // verifiers consistently.
            let prompt_text = input
                .messages
                .iter()
                .filter_map(|m| {
                    m.content
                        .iter()
                        .filter_map(ContentPart::as_text)
                        .next()
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
                .join("\n");

            let mut messages: Vec<Message> = Vec::new();
            if let Some(sys) = input.system.clone() {
                messages.push(Message::system(sys));
            }
            messages.extend(input.messages);

            let candidates: Vec<String> = role_order
                .iter()
                .map(|r| {
                    let pid = roles.get(r).map(|p| p.id().to_string()).unwrap_or_default();
                    format!("{r}|{pid}")
                })
                .collect();

            let mut total_usage = Usage::default();
            let mut steps = 0_u32;
            let mut final_message: Option<Message> = None;

            for step in 0..max_steps {
                yield OrchEvent::StepStart { step };

                let req_for_router = ChatRequest::new("router", messages.clone());
                let decision = router.route(&principal, &req_for_router, &candidates).await?;
                let (role, provider) = pick_provider_static(&roles, &decision.provider_id)
                    .ok_or_else(|| {
                        TakoError::Invalid(format!(
                            "Trinity: router chose unknown provider id `{}`",
                            decision.provider_id
                        ))
                    })?;

                let model = provider.id().split(':').nth(1).unwrap_or("").to_string();
                let req = ChatRequest {
                    model: model.clone(),
                    messages: messages.clone(),
                    tools: tools.schemas().await,
                    temperature: defaults.temperature,
                    max_tokens: defaults.max_tokens,
                    stop: Vec::new(),
                    stream: provider.capabilities().supports_streaming,
                    metadata: Default::default(),
                };

                // Budget pre-check covers the streaming branch and both
                // non-streaming fallback paths below; one record() runs
                // after the branch picks a path.
                let estimated_usd = provider.estimate_cost_usd(&req);
                if let Some(b) = budget.as_ref() {
                    let est_tokens = req.max_tokens.unwrap_or(0);
                    b.pre_check(&principal, estimated_usd, est_tokens).await?;
                }

                let span = info_span!(
                    "tako.provider.chat",
                    "tako.provider.id" = %provider.id(),
                    "tako.provider.model" = %model,
                    "tako.orchestrator.step" = step,
                    "tako.orchestrator.role" = %role,
                );

                let (assistant, finish_reason, step_usage) = if provider
                    .capabilities()
                    .supports_streaming
                {
                    let stream_result = provider.stream(&principal, req).instrument(span).await;
                    match stream_result {
                        Ok(mut chunks) => {
                            let mut text = String::new();
                            let mut deltas: Vec<ToolCallDelta> = Vec::new();
                            let mut finish = FinishReason::Stop;
                            let mut usage = Usage::default();
                            while let Some(chunk) = chunks.next().await {
                                match chunk? {
                                    ChatChunk::Delta { text: t, tool_calls } => {
                                        if let Some(t) = t {
                                            if !t.is_empty() {
                                                yield OrchEvent::AssistantText {
                                                    step,
                                                    delta: t.clone(),
                                                };
                                                text.push_str(&t);

                                                // Phase 13.B — per-delta verifier
                                                // hook on the cumulative buffer.
                                                // Default trait impl returns
                                                // Ok(None) so unmodified verifiers
                                                // emit no streaming partials and
                                                // behaviour is unchanged. Override
                                                // for cheap heuristic verifiers
                                                // (regex, length); skip for
                                                // LLM-as-judge.
                                                if let Some(v) = verifier.as_ref() {
                                                    if let Some(score) = v
                                                        .evaluate_streaming(
                                                            &principal, &text,
                                                        )
                                                        .await?
                                                    {
                                                        let branch = role_order
                                                            .iter()
                                                            .position(|r| r == &role)
                                                            .unwrap_or(0)
                                                            as u32;
                                                        yield OrchEvent::VerifierScore {
                                                            step,
                                                            branch,
                                                            score: score
                                                                .clamp(0.0, 1.0),
                                                        };
                                                    }
                                                }
                                            }
                                        }
                                        deltas.extend(tool_calls);
                                    }
                                    ChatChunk::Error { message } => {
                                        Err(TakoError::provider(
                                            provider.id(),
                                            &model,
                                            format!("stream error: {message}"),
                                        ))?;
                                    }
                                    ChatChunk::End { finish_reason: fr, usage: u } => {
                                        finish = fr;
                                        usage = u;
                                    }
                                }
                            }
                            let mut content: Vec<ContentPart> = Vec::new();
                            if !text.is_empty() {
                                content.push(ContentPart::Text { text });
                            }
                            for tc in assemble_tool_calls_pub(deltas) {
                                content.push(tc);
                            }
                            (Message { role: Role::Assistant, content }, finish, usage)
                        }
                        Err(_) => {
                            // Provider claimed streaming but failed to start;
                            // fall back to non-streaming.
                            let req2 = ChatRequest {
                                model: model.clone(),
                                messages: messages.clone(),
                                tools: tools.schemas().await,
                                temperature: defaults.temperature,
                                max_tokens: defaults.max_tokens,
                                stop: Vec::new(),
                                stream: false,
                                metadata: Default::default(),
                            };
                            let resp = provider.chat(&principal, req2).await?;
                            let text_delta = resp
                                .message
                                .content
                                .iter()
                                .filter_map(ContentPart::as_text)
                                .collect::<Vec<_>>()
                                .join("");
                            if !text_delta.is_empty() {
                                yield OrchEvent::AssistantText {
                                    step,
                                    delta: text_delta,
                                };
                            }
                            (resp.message, resp.finish_reason, resp.usage)
                        }
                    }
                } else {
                    let req2 = ChatRequest {
                        model: model.clone(),
                        messages: messages.clone(),
                        tools: tools.schemas().await,
                        temperature: defaults.temperature,
                        max_tokens: defaults.max_tokens,
                        stop: Vec::new(),
                        stream: false,
                        metadata: Default::default(),
                    };
                    let resp = provider.chat(&principal, req2).instrument(span).await?;
                    let text_delta = resp
                        .message
                        .content
                        .iter()
                        .filter_map(ContentPart::as_text)
                        .collect::<Vec<_>>()
                        .join("");
                    if !text_delta.is_empty() {
                        yield OrchEvent::AssistantText {
                            step,
                            delta: text_delta,
                        };
                    }
                    (resp.message, resp.finish_reason, resp.usage)
                };

                if let Some(b) = budget.as_ref() {
                    b.record(&principal, estimated_usd, step_usage).await?;
                }
                steps += 1;
                total_usage.input_tokens = total_usage
                    .input_tokens
                    .saturating_add(step_usage.input_tokens);
                total_usage.output_tokens = total_usage
                    .output_tokens
                    .saturating_add(step_usage.output_tokens);

                // Phase 10.C — score the role's assistant text once
                // the turn is complete. `branch` is the role's
                // positional index in `role_order` so consumers can
                // attribute the score to the specific role/provider
                // that produced this turn. No emission when no
                // verifier is attached.
                if let Some(v) = verifier.as_ref() {
                    let assistant_text = assistant
                        .content
                        .iter()
                        .filter_map(ContentPart::as_text)
                        .collect::<Vec<_>>()
                        .join("");
                    let branch = role_order
                        .iter()
                        .position(|r| r == &role)
                        .unwrap_or(0) as u32;
                    let score = v
                        .score(&principal, &prompt_text, &assistant_text)
                        .await?
                        .clamp(0.0, 1.0);
                    yield OrchEvent::VerifierScore { step, branch, score };
                }

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
                    || matches!(finish_reason, FinishReason::Stop | FinishReason::Length)
                        && tool_calls.is_empty()
                {
                    final_message = Some(assistant);
                    break;
                }

                let mut tool_results: Vec<ContentPart> = Vec::with_capacity(tool_calls.len());
                for (id, name, args) in tool_calls {
                    if let Some(engine) = &policy {
                        let ctx = PolicyContext {
                            stage: PolicyStage::PreTool,
                            model: model.clone(),
                            messages_hash: hash_messages_pub(&messages),
                            tools: vec![name.clone()],
                            tool_args_hash: Some(hash_value_pub(&args)),
                            response_hash: None,
                        };
                        match engine.evaluate(&principal, ctx).await? {
                            PolicyDecision::Deny { reason } => {
                                Err(TakoError::PolicyDenied(format!("tool `{name}`: {reason}")))?;
                            }
                            PolicyDecision::RequireApproval { reason } => {
                                Err(TakoError::PolicyDenied(format!(
                                    "tool `{name}` requires approval: {reason}"
                                )))?;
                            }
                            _ => {}
                        }
                    }
                    yield OrchEvent::ToolCallStart {
                        step,
                        name: name.clone(),
                        id: id.clone(),
                    };
                    let (result_part, event_value, is_error) =
                        match tools.invoke(&principal, &name, args).await {
                            Ok(v) => (
                                ContentPart::ToolResult {
                                    id: id.clone(),
                                    result: v.clone(),
                                    is_error: false,
                                },
                                v,
                                false,
                            ),
                            Err(e) => {
                                let err = serde_json::json!({ "error": e.to_string() });
                                (
                                    ContentPart::ToolResult {
                                        id: id.clone(),
                                        result: err.clone(),
                                        is_error: true,
                                    },
                                    err,
                                    true,
                                )
                            }
                        };
                    yield OrchEvent::ToolCallResult {
                        step,
                        id: id.clone(),
                        result: event_value,
                        is_error,
                    };
                    tool_results.push(result_part);
                }
                messages.push(Message {
                    role: Role::User,
                    content: tool_results,
                });

                if step + 1 == max_steps {
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
            yield OrchEvent::Final {
                output: Box::new(OrchOutput {
                    text,
                    message: final_message,
                    usage: total_usage,
                    steps,
                }),
            };
        };

        Box::pin(s)
    }
}

/// Stream-loop variant of `Trinity::pick_provider`. Captures only owned
/// data so it can run inside `async_stream::try_stream!` where `&self`
/// is unavailable.
fn pick_provider_static(
    roles: &HashMap<String, Arc<dyn LlmProvider>>,
    decision_id: &str,
) -> Option<(String, Arc<dyn LlmProvider>)> {
    if let Some((role, _pid)) = decision_id.split_once('|') {
        if let Some(p) = roles.get(role) {
            return Some((role.to_string(), p.clone()));
        }
    }
    if let Some(p) = roles.get(decision_id) {
        return Some((decision_id.to_string(), p.clone()));
    }
    for (role, p) in roles {
        if p.id() == decision_id {
            return Some((role.clone(), p.clone()));
        }
    }
    None
}
