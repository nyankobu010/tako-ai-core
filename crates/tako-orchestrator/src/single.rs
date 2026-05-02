//! `SingleAgent` orchestrator: one provider + a tool registry + a
//! max-step loop. Halts on a model response with no tool calls (final
//! answer) or when `max_steps` is reached.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use sha2::{Digest, Sha256};
use tako_core::{
    ChatChunk, ChatRequest, ChatResponse, ContentPart, FinishReason, LlmProvider, Message,
    PolicyContext, PolicyDecision, PolicyEngine, PolicyStage, Principal, Role, Router, TakoError,
    ToolCallDelta, Usage,
};
use tako_mcp::ToolRegistry;
use tako_runtime::BudgetTracker;
use tracing::{Instrument, info_span};

use crate::types::{OrchEvent, OrchInput, OrchOutput};
use crate::{Orchestrator, OrchestratorKind};

const DEFAULT_MAX_STEPS: u32 = 8;

/// Single-agent orchestrator.
pub struct SingleAgent {
    provider: Arc<dyn LlmProvider>,
    /// Optional additional candidates routed among via `router`. When
    /// `router` is set, each step calls `router.route(...)` over
    /// `[provider, ...candidates]` and uses the chosen provider for that
    /// turn's chat call. Without a router, `provider` is always used.
    candidates: Vec<Arc<dyn LlmProvider>>,
    router: Option<Arc<dyn Router>>,
    tools: Arc<ToolRegistry>,
    max_steps: u32,
    /// Optional pre-set defaults for ChatRequest fields the orchestrator
    /// constructs (temperature, max_tokens). Tools and stream are managed
    /// by the orchestrator itself.
    defaults: ChatDefaults,
    /// Optional policy engine consulted at PreChat / PreTool stages.
    /// `None` is equivalent to AllowAll (zero overhead).
    policy: Option<Arc<dyn PolicyEngine>>,
    /// Optional budget tracker consulted before each provider call
    /// (`pre_check`) and after each call (`record`). When `None`, no
    /// budget enforcement runs and the orchestrator behaves exactly as
    /// in v0.5.0.
    budget: Option<Arc<BudgetTracker>>,
}

#[derive(Clone, Debug, Default)]
pub struct ChatDefaults {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl std::fmt::Debug for SingleAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleAgent")
            .field("provider", &self.provider.id())
            .field("max_steps", &self.max_steps)
            .finish()
    }
}

impl SingleAgent {
    pub fn builder() -> SingleAgentBuilder {
        SingleAgentBuilder::default()
    }
}

#[derive(Default)]
pub struct SingleAgentBuilder {
    provider: Option<Arc<dyn LlmProvider>>,
    candidates: Vec<Arc<dyn LlmProvider>>,
    router: Option<Arc<dyn Router>>,
    tools: Option<Arc<ToolRegistry>>,
    max_steps: Option<u32>,
    defaults: ChatDefaults,
    policy: Option<Arc<dyn PolicyEngine>>,
    budget: Option<Arc<BudgetTracker>>,
}

impl std::fmt::Debug for SingleAgentBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleAgentBuilder")
            .field("provider", &self.provider.as_ref().map(|p| p.id()))
            .field("max_steps", &self.max_steps)
            .field("defaults", &self.defaults)
            .finish_non_exhaustive()
    }
}

impl SingleAgentBuilder {
    pub fn provider(mut self, provider: Arc<dyn LlmProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = Some(tools);
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

    /// Attach a policy engine consulted before each provider call
    /// (PreChat) and before each tool invocation (PreTool). A
    /// `Deny` decision is propagated as `TakoError::PolicyDenied`.
    pub fn policy(mut self, policy: Arc<dyn PolicyEngine>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Add an additional candidate provider that the optional `router`
    /// can pick alongside the primary `provider`. Calling this without
    /// also calling [`Self::router`] has no effect at runtime — the
    /// primary will be used unconditionally.
    pub fn candidate(mut self, p: Arc<dyn LlmProvider>) -> Self {
        self.candidates.push(p);
        self
    }

    /// Attach a `Router` that picks among `[provider, ...candidates]`
    /// at each step. Without a router, the primary provider is used
    /// unconditionally; this preserves backwards compatibility.
    pub fn router(mut self, r: Arc<dyn Router>) -> Self {
        self.router = Some(r);
        self
    }

    /// Attach a [`BudgetTracker`]. When set, the orchestrator calls
    /// `pre_check` before each provider invocation and `record` after,
    /// using the provider's [`tako_core::LlmProvider::estimate_cost_usd`]
    /// for both the pre-flight estimate and the post-call cost
    /// (provider crates do not yet expose actual-rate cost on `Usage`).
    /// A `BudgetExhausted` error short-circuits the run.
    pub fn budget(mut self, t: Arc<BudgetTracker>) -> Self {
        self.budget = Some(t);
        self
    }

    pub fn build(self) -> Result<SingleAgent, TakoError> {
        Ok(SingleAgent {
            provider: self.provider.ok_or_else(|| {
                TakoError::Invalid("SingleAgentBuilder: provider is required".into())
            })?,
            candidates: self.candidates,
            router: self.router,
            tools: self.tools.unwrap_or_else(|| Arc::new(ToolRegistry::new())),
            max_steps: self.max_steps.unwrap_or(DEFAULT_MAX_STEPS),
            defaults: self.defaults,
            policy: self.policy,
            budget: self.budget,
        })
    }
}

impl SingleAgent {
    /// Pick the provider for this step. With no router attached, always
    /// returns the primary provider. With a router, calls
    /// `router.route(principal, &req, &candidate_ids)` over
    /// `[primary, ...candidates]` and returns whichever one matches.
    async fn pick_provider(
        &self,
        principal: &Principal,
        messages: &[Message],
    ) -> Result<Arc<dyn LlmProvider>, TakoError> {
        let Some(router) = &self.router else {
            return Ok(self.provider.clone());
        };
        if self.candidates.is_empty() {
            return Ok(self.provider.clone());
        }
        let pool: Vec<Arc<dyn LlmProvider>> = std::iter::once(self.provider.clone())
            .chain(self.candidates.iter().cloned())
            .collect();
        let candidate_ids: Vec<String> = pool.iter().map(|p| p.id().to_string()).collect();
        let req = ChatRequest::new("router", messages.to_vec());
        let decision = router.route(principal, &req, &candidate_ids).await?;
        pool.into_iter()
            .find(|p| p.id() == decision.provider_id)
            .ok_or_else(|| {
                TakoError::Invalid(format!(
                    "SingleAgent: router chose unknown provider id `{}`",
                    decision.provider_id
                ))
            })
    }

    fn build_request(
        &self,
        model: &str,
        messages: Vec<Message>,
        tool_schemas: Vec<tako_core::ToolSchema>,
    ) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages,
            tools: tool_schemas,
            temperature: self.defaults.temperature,
            max_tokens: self.defaults.max_tokens,
            stop: Vec::new(),
            stream: false,
            metadata: Default::default(),
        }
    }
}

#[async_trait]
impl Orchestrator for SingleAgent {
    fn kind(&self) -> OrchestratorKind {
        OrchestratorKind::SingleAgent
    }

    async fn run(&self, principal: &Principal, input: OrchInput) -> Result<OrchOutput, TakoError> {
        let span = info_span!(
            "tako.orchestrator.run",
            "tako.orchestrator.kind" = "single",
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

            for step in 0..self.max_steps {
                let provider = self.pick_provider(principal, &messages).await?;
                let model = provider.id().split(':').nth(1).unwrap_or("").to_string();
                let tool_schemas = self.tools.schemas().await;
                let req = self.build_request(&model, messages.clone(), tool_schemas);

                // Pre-check: estimate cost + token budget before the call.
                // Pessimistic; per-provider rates aren't on the trait so
                // we reuse the same estimate post-call.
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

                // Execute each tool call in order; append results as a single
                // user-role message containing all ToolResult parts. PreTool
                // policy is consulted per call.
                let mut tool_results: Vec<ContentPart> = Vec::with_capacity(tool_calls.len());
                for (id, name, args) in tool_calls {
                    if let Some(engine) = &self.policy {
                        let ctx = PolicyContext {
                            stage: PolicyStage::PreTool,
                            model: model.clone(),
                            messages_hash: hash_messages(&messages),
                            tools: vec![name.clone()],
                            tool_args_hash: Some(hash_value(&args)),
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
        let primary = self.provider.clone();
        let candidates = self.candidates.clone();
        let router = self.router.clone();
        let tools = self.tools.clone();
        let policy = self.policy.clone();
        let defaults = self.defaults.clone();
        let max_steps = self.max_steps;
        let principal = principal.clone();
        let budget = self.budget.clone();

        let s = async_stream::try_stream! {
            // Per-step provider spans are emitted inside the loop. We do not
            // wrap the whole stream in `tako.orchestrator.run` because the
            // tracing `Instrument` trait is for futures, not streams; the
            // step spans capture the telemetry consumers actually need.
            let mut messages: Vec<Message> = Vec::new();
            if let Some(sys) = input.system.clone() {
                messages.push(Message::system(sys));
            }
            messages.extend(input.messages);

            let mut total_usage = Usage::default();
            let mut steps = 0_u32;
            let mut final_message: Option<Message> = None;

            for step in 0..max_steps {
                yield OrchEvent::StepStart { step };

                let provider = pick_provider_static(
                    &primary,
                    &candidates,
                    router.as_ref(),
                    &principal,
                    &messages,
                )
                .await?;
                let model = provider.id().split(':').nth(1).unwrap_or("").to_string();
                let tool_schemas = tools.schemas().await;
                let req = ChatRequest {
                    model: model.clone(),
                    messages: messages.clone(),
                    tools: tool_schemas,
                    temperature: defaults.temperature,
                    max_tokens: defaults.max_tokens,
                    stop: Vec::new(),
                    stream: provider.capabilities().supports_streaming,
                    metadata: Default::default(),
                };

                // Budget pre-check before either streaming or buffered call.
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
                            for tc in assemble_tool_calls(deltas) {
                                content.push(tc);
                            }
                            (
                                Message { role: Role::Assistant, content },
                                finish,
                                usage,
                            )
                        }
                        Err(_) => {
                            // Provider claimed streaming but failed to start;
                            // fall back to non-streaming.
                            let resp = chat_fallback(&provider, &principal, &model, &messages, &tools, &defaults).await?;
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
                    let resp = chat_fallback(&provider, &principal, &model, &messages, &tools, &defaults).await?;
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
                            messages_hash: hash_messages(&messages),
                            tools: vec![name.clone()],
                            tool_args_hash: Some(hash_value(&args)),
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

/// Reassemble streaming `ToolCallDelta` increments into final
/// `ContentPart::ToolCall` parts. Deltas are keyed by `index`.
fn assemble_tool_calls(deltas: Vec<ToolCallDelta>) -> Vec<ContentPart> {
    use std::collections::BTreeMap;
    let mut acc: BTreeMap<u32, (Option<String>, Option<String>, String)> = BTreeMap::new();
    for d in deltas {
        let entry = acc.entry(d.index).or_insert((None, None, String::new()));
        if let Some(id) = d.id {
            entry.0 = Some(id);
        }
        if let Some(name) = d.name {
            entry.1 = Some(name);
        }
        if let Some(arg) = d.arguments_fragment {
            entry.2.push_str(&arg);
        }
    }
    acc.into_iter()
        .filter_map(|(_, (id, name, args_str))| {
            let id = id?;
            let name = name?;
            let args = if args_str.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}))
            };
            Some(ContentPart::ToolCall { id, name, args })
        })
        .collect()
}

/// Stream-loop variant of `pick_provider`. Captures only owned data so it
/// can run inside `async_stream::try_stream!` where `&self` is unavailable.
async fn pick_provider_static(
    primary: &Arc<dyn LlmProvider>,
    candidates: &[Arc<dyn LlmProvider>],
    router: Option<&Arc<dyn Router>>,
    principal: &Principal,
    messages: &[Message],
) -> Result<Arc<dyn LlmProvider>, TakoError> {
    let Some(router) = router else {
        return Ok(primary.clone());
    };
    if candidates.is_empty() {
        return Ok(primary.clone());
    }
    let pool: Vec<Arc<dyn LlmProvider>> = std::iter::once(primary.clone())
        .chain(candidates.iter().cloned())
        .collect();
    let candidate_ids: Vec<String> = pool.iter().map(|p| p.id().to_string()).collect();
    let req = ChatRequest::new("router", messages.to_vec());
    let decision = router.route(principal, &req, &candidate_ids).await?;
    pool.into_iter()
        .find(|p| p.id() == decision.provider_id)
        .ok_or_else(|| {
            TakoError::Invalid(format!(
                "SingleAgent: router chose unknown provider id `{}`",
                decision.provider_id
            ))
        })
}

async fn chat_fallback(
    provider: &Arc<dyn LlmProvider>,
    principal: &Principal,
    model: &str,
    messages: &[Message],
    tools: &Arc<ToolRegistry>,
    defaults: &ChatDefaults,
) -> Result<ChatResponse, TakoError> {
    let req = ChatRequest {
        model: model.to_string(),
        messages: messages.to_vec(),
        tools: tools.schemas().await,
        temperature: defaults.temperature,
        max_tokens: defaults.max_tokens,
        stop: Vec::new(),
        stream: false,
        metadata: Default::default(),
    };
    provider.chat(principal, req).await
}

pub(crate) fn hash_messages_pub(m: &[Message]) -> String {
    hash_messages(m)
}

pub(crate) fn assemble_tool_calls_pub(deltas: Vec<ToolCallDelta>) -> Vec<ContentPart> {
    assemble_tool_calls(deltas)
}

pub(crate) fn hash_value_pub(v: &serde_json::Value) -> String {
    hash_value(v)
}

fn hash_messages(messages: &[Message]) -> String {
    let mut hasher = Sha256::new();
    for m in messages {
        hasher.update(format!("{:?}", m.role).as_bytes());
        for c in &m.content {
            hasher.update(serde_json::to_string(c).unwrap_or_default().as_bytes());
        }
        hasher.update(b"|");
    }
    hex_digest(hasher.finalize().as_slice())
}

fn hash_value(v: &serde_json::Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex_digest(hasher.finalize().as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
