//! AB-MCTS — Adaptive Branching Monte Carlo Tree Search.
//!
//! Generalisation of Inoue et al., *AB-MCTS* (arXiv:2503.04412). Each
//! iteration of the search loop:
//!
//! 1. **Selection** — descend from the root, Thompson-sampling each
//!    child's `Beta(α, β)` posterior and picking the argmax.
//! 2. **Adaptive branching** — at every node we also draw a Thompson
//!    sample for a hypothetical *new* child (initialised with a weak
//!    `Beta(1, 1)` prior). If the new-child sample beats every existing
//!    child, expand a new sibling instead of descending. This is the
//!    "AB" in AB-MCTS.
//! 3. **Expansion** — generate one rollout from the chosen branch via
//!    a `SingleAgent`-style step loop bounded by `max_steps_per_rollout`.
//! 4. **Verification** — score the rollout's output text on `[0, 1]`
//!    via the user-provided [`tako_core::Verifier`].
//! 5. **Back-propagation** — update Beta posteriors on every node along
//!    the path: `α += score`, `β += 1 - score`.
//!
//! Termination: `max_iterations` budget OR `min_confidence` threshold
//! satisfied by the best leaf observed so far. The orchestrator
//! returns the highest-scored leaf as `OrchOutput`.
//!
//! OTel: root `tako.orchestrator.run` with
//! `tako.orchestrator.kind = "ab_mcts"`; per-iteration child span
//! `tako.ab_mcts.iteration` with `tako.ab_mcts.depth`,
//! `tako.ab_mcts.score`, `tako.ab_mcts.posterior_alpha`,
//! `tako.ab_mcts.posterior_beta`.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tako_core::{
    ChatRequest, ContentPart, FinishReason, LlmProvider, Message, PolicyContext, PolicyDecision,
    PolicyEngine, PolicyStage, Principal, Role, TakoError, Usage, Verifier,
};
use tako_mcp::ToolRegistry;
use tracing::{Instrument, info_span};

use crate::single::{ChatDefaults, hash_messages_pub, hash_value_pub};
use crate::types::{OrchEvent, OrchInput, OrchOutput};
use crate::{Orchestrator, OrchestratorKind};

const DEFAULT_MAX_ITERATIONS: u32 = 16;
const DEFAULT_BRANCHING_FACTOR: u32 = 3;
const DEFAULT_MAX_STEPS_PER_ROLLOUT: u32 = 4;
const DEFAULT_TEMPERATURE: f32 = 0.7;
const DEFAULT_MIN_CONFIDENCE: f32 = 0.95;

/// AB-MCTS orchestrator.
pub struct AbMcts {
    provider: Arc<dyn LlmProvider>,
    verifier: Arc<dyn Verifier>,
    tools: Arc<ToolRegistry>,
    policy: Option<Arc<dyn PolicyEngine>>,
    defaults: ChatDefaults,
    max_iterations: u32,
    branching_factor: u32,
    max_steps_per_rollout: u32,
    temperature: f32,
    min_confidence: f32,
}

impl std::fmt::Debug for AbMcts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbMcts")
            .field("provider", &self.provider.id())
            .field("max_iterations", &self.max_iterations)
            .field("branching_factor", &self.branching_factor)
            .field("max_steps_per_rollout", &self.max_steps_per_rollout)
            .field("temperature", &self.temperature)
            .field("min_confidence", &self.min_confidence)
            .finish()
    }
}

impl AbMcts {
    pub fn builder() -> AbMctsBuilder {
        AbMctsBuilder::default()
    }
}

#[derive(Default)]
pub struct AbMctsBuilder {
    provider: Option<Arc<dyn LlmProvider>>,
    verifier: Option<Arc<dyn Verifier>>,
    tools: Option<Arc<ToolRegistry>>,
    policy: Option<Arc<dyn PolicyEngine>>,
    defaults: ChatDefaults,
    max_iterations: Option<u32>,
    branching_factor: Option<u32>,
    max_steps_per_rollout: Option<u32>,
    temperature: Option<f32>,
    min_confidence: Option<f32>,
}

impl std::fmt::Debug for AbMctsBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbMctsBuilder").finish_non_exhaustive()
    }
}

impl AbMctsBuilder {
    pub fn provider(mut self, p: Arc<dyn LlmProvider>) -> Self {
        self.provider = Some(p);
        self
    }

    pub fn verifier(mut self, v: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(v);
        self
    }

    pub fn tools(mut self, t: Arc<ToolRegistry>) -> Self {
        self.tools = Some(t);
        self
    }

    pub fn policy(mut self, p: Arc<dyn PolicyEngine>) -> Self {
        self.policy = Some(p);
        self
    }

    pub fn max_iterations(mut self, n: u32) -> Self {
        self.max_iterations = Some(n.max(1));
        self
    }

    pub fn branching_factor(mut self, k: u32) -> Self {
        self.branching_factor = Some(k.max(1));
        self
    }

    pub fn max_steps_per_rollout(mut self, n: u32) -> Self {
        self.max_steps_per_rollout = Some(n.max(1));
        self
    }

    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn max_tokens(mut self, n: u32) -> Self {
        self.defaults.max_tokens = Some(n);
        self
    }

    /// Stop early when the best leaf's verifier score reaches this
    /// threshold. Default: `0.95`.
    pub fn min_confidence(mut self, c: f32) -> Self {
        self.min_confidence = Some(c.clamp(0.0, 1.0));
        self
    }

    pub fn build(self) -> Result<AbMcts, TakoError> {
        let provider = self
            .provider
            .ok_or_else(|| TakoError::Invalid("AbMctsBuilder: provider is required".into()))?;
        let verifier = self
            .verifier
            .ok_or_else(|| TakoError::Invalid("AbMctsBuilder: verifier is required".into()))?;
        let temperature = self.temperature.unwrap_or(DEFAULT_TEMPERATURE);
        let mut defaults = self.defaults;
        // Sampling temperature defaults to >0 so rollouts are diverse.
        if defaults.temperature.is_none() {
            defaults.temperature = Some(temperature);
        }
        Ok(AbMcts {
            provider,
            verifier,
            tools: self.tools.unwrap_or_else(|| Arc::new(ToolRegistry::new())),
            policy: self.policy,
            defaults,
            max_iterations: self.max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS),
            branching_factor: self.branching_factor.unwrap_or(DEFAULT_BRANCHING_FACTOR),
            max_steps_per_rollout: self
                .max_steps_per_rollout
                .unwrap_or(DEFAULT_MAX_STEPS_PER_ROLLOUT),
            temperature,
            min_confidence: self.min_confidence.unwrap_or(DEFAULT_MIN_CONFIDENCE),
        })
    }
}

/// One node in the search tree. The root represents the initial prompt;
/// each non-root node represents one rollout extending its parent's
/// conversation.
#[derive(Debug)]
struct Node {
    /// Conversation prefix at this node. The root holds the user's
    /// prompt; children append assistant turns produced by rollouts.
    messages: Vec<Message>,
    /// Beta posterior for this node's branch quality.
    alpha: f32,
    beta: f32,
    /// Indices into the `nodes` arena.
    children: Vec<usize>,
    /// Final-message text of the rollout that reached this leaf.
    /// `None` for the root and intermediate nodes.
    output_text: Option<String>,
    /// Final assistant message of the leaf rollout (if any).
    output_message: Option<Message>,
    /// Number of model steps the rollout consumed.
    rollout_steps: u32,
}

impl Node {
    fn new_root(messages: Vec<Message>) -> Self {
        Self {
            messages,
            alpha: 1.0,
            beta: 1.0,
            children: Vec::new(),
            output_text: None,
            output_message: None,
            rollout_steps: 0,
        }
    }
}

#[async_trait]
impl Orchestrator for AbMcts {
    fn kind(&self) -> OrchestratorKind {
        OrchestratorKind::AbMcts
    }

    async fn run(&self, principal: &Principal, input: OrchInput) -> Result<OrchOutput, TakoError> {
        let span = info_span!(
            "tako.orchestrator.run",
            "tako.orchestrator.kind" = "ab_mcts",
            "tako.principal.tenant_id" = %principal.tenant_id,
            "tako.principal.user_id" = %principal.user_id,
        );
        async move {
            let mut root_messages: Vec<Message> = Vec::new();
            if let Some(sys) = input.system.clone() {
                root_messages.push(Message::system(sys));
            }
            root_messages.extend(input.messages.clone());

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

            let mut nodes: Vec<Node> = vec![Node::new_root(root_messages.clone())];
            let mut best_leaf: Option<usize> = None;
            let mut best_score: f32 = -1.0;
            let mut total_usage = Usage::default();
            // Seed the search RNG from the thread-local generator so each
            // run is non-deterministic, but the future is `Send` (StdRng is,
            // ThreadRng isn't).
            let mut rng = StdRng::from_rng(&mut rand::rng());

            for iteration in 0..self.max_iterations {
                let iter_span = info_span!(
                    "tako.ab_mcts.iteration",
                    "tako.ab_mcts.iteration" = iteration,
                );
                let outcome = self
                    .iterate(&mut nodes, &mut rng, principal, &prompt_text)
                    .instrument(iter_span)
                    .await?;
                total_usage.input_tokens = total_usage
                    .input_tokens
                    .saturating_add(outcome.rollout_usage.input_tokens);
                total_usage.output_tokens = total_usage
                    .output_tokens
                    .saturating_add(outcome.rollout_usage.output_tokens);

                if outcome.score > best_score {
                    best_score = outcome.score;
                    best_leaf = Some(outcome.leaf);
                }
                if best_score >= self.min_confidence {
                    break;
                }
            }

            let leaf_idx = best_leaf.ok_or_else(|| {
                TakoError::Invalid(
                    "AbMcts: no rollout completed (try increasing max_iterations)".into(),
                )
            })?;
            let leaf = &nodes[leaf_idx];
            let final_message = leaf
                .output_message
                .clone()
                .unwrap_or_else(|| Message::assistant(""));
            let text = leaf.output_text.clone().unwrap_or_default();
            let total_steps = leaf.rollout_steps;
            Ok(OrchOutput {
                text,
                message: final_message,
                usage: total_usage,
                steps: total_steps,
            })
        }
        .instrument(span)
        .await
    }

    /// Streaming variant of [`AbMcts::run`] (Phase 8.B).
    ///
    /// Per iteration of the AB-MCTS search loop, the stream emits:
    /// 1. [`OrchEvent::StepStart`] with `step = iteration_index`.
    /// 2. Exactly one [`OrchEvent::AssistantText`] carrying the full
    ///    rollout text — the rollout helper itself is non-streaming
    ///    (each rollout may invoke tool calls across multiple
    ///    provider turns; per-token interleaving across competing
    ///    branches is out of scope for v0.9.0).
    /// 3. [`OrchEvent::VerifierScore`] with the rollout's branch
    ///    index (the leaf node id) and verifier score in `[0, 1]`.
    ///
    /// After all iterations (or after early-stop on
    /// `score >= min_confidence`), the stream emits exactly one
    /// terminal [`OrchEvent::Final`] containing the highest-scored
    /// leaf's `OrchOutput`, matching `run`'s return value.
    async fn stream(
        &self,
        principal: &Principal,
        input: OrchInput,
    ) -> BoxStream<'static, Result<OrchEvent, TakoError>> {
        let provider = Arc::clone(&self.provider);
        let verifier = Arc::clone(&self.verifier);
        let tools = Arc::clone(&self.tools);
        let policy = self.policy.clone();
        let defaults = self.defaults.clone();
        let max_iterations = self.max_iterations;
        let branching_factor = self.branching_factor;
        let max_steps_per_rollout = self.max_steps_per_rollout;
        let temperature = self.temperature;
        let min_confidence = self.min_confidence;
        let principal = principal.clone();

        let s = async_stream::try_stream! {
            let mut root_messages: Vec<Message> = Vec::new();
            if let Some(sys) = input.system.clone() {
                root_messages.push(Message::system(sys));
            }
            root_messages.extend(input.messages.clone());

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

            let mut nodes: Vec<Node> = vec![Node::new_root(root_messages.clone())];
            let mut best_leaf: Option<usize> = None;
            let mut best_score: f32 = -1.0;
            let mut total_usage = Usage::default();
            let mut rng = StdRng::from_rng(&mut rand::rng());

            for iteration in 0..max_iterations {
                yield OrchEvent::StepStart { step: iteration };

                // ---- Selection / adaptive branching (mirrors `iterate`) ----
                let mut current = 0usize;
                let mut path: Vec<usize> = vec![current];
                loop {
                    let node = &nodes[current];
                    let new_child_sample = sample_beta(&mut rng, 1.0, 1.0);
                    let mut best_child: Option<usize> = None;
                    let mut best_child_sample = f32::NEG_INFINITY;
                    for &c in &node.children {
                        let c_node = &nodes[c];
                        let s = sample_beta(&mut rng, c_node.alpha, c_node.beta);
                        if s > best_child_sample {
                            best_child_sample = s;
                            best_child = Some(c);
                        }
                    }
                    let can_branch = (node.children.len() as u32) < branching_factor;
                    let should_expand = match best_child {
                        None => true,
                        Some(_) => can_branch && new_child_sample > best_child_sample,
                    };
                    if should_expand {
                        break;
                    }
                    let Some(next) = best_child else { break };
                    path.push(next);
                    current = next;
                }

                // ---- Expansion + simulation (rollout) ----
                let parent_idx = current;
                let parent_messages = nodes[parent_idx].messages.clone();
                let (rollout_message, rollout_text, rollout_usage, rollout_steps) =
                    rollout_static(
                        Arc::clone(&provider),
                        Arc::clone(&tools),
                        policy.clone(),
                        defaults.clone(),
                        max_steps_per_rollout,
                        temperature,
                        &principal,
                        parent_messages.clone(),
                    )
                    .await?;

                // Forward the rollout's full text as a single
                // AssistantText delta. Per-token streaming inside a
                // multi-step rollout is deferred (would need to thread
                // `provider.stream` through the tool-call loop).
                if !rollout_text.is_empty() {
                    yield OrchEvent::AssistantText {
                        step: iteration,
                        delta: rollout_text.clone(),
                    };
                }

                let mut child_messages = parent_messages;
                child_messages.push(rollout_message.clone());

                // ---- Verification ----
                let score = verifier
                    .score(&principal, &prompt_text, &rollout_text)
                    .await?
                    .clamp(0.0, 1.0);

                // ---- Insert leaf ----
                let leaf = Node {
                    messages: child_messages,
                    alpha: 1.0,
                    beta: 1.0,
                    children: Vec::new(),
                    output_text: Some(rollout_text),
                    output_message: Some(rollout_message),
                    rollout_steps,
                };
                let leaf_idx = nodes.len();
                nodes.push(leaf);
                nodes[parent_idx].children.push(leaf_idx);
                path.push(leaf_idx);

                yield OrchEvent::VerifierScore {
                    step: iteration,
                    branch: leaf_idx as u32,
                    score,
                };

                // ---- Back-propagation ----
                for &n in &path {
                    nodes[n].alpha += score;
                    nodes[n].beta += 1.0 - score;
                }

                total_usage.input_tokens =
                    total_usage.input_tokens.saturating_add(rollout_usage.input_tokens);
                total_usage.output_tokens =
                    total_usage.output_tokens.saturating_add(rollout_usage.output_tokens);

                if score > best_score {
                    best_score = score;
                    best_leaf = Some(leaf_idx);
                }
                if best_score >= min_confidence {
                    break;
                }
            }

            let leaf_idx = best_leaf.ok_or_else(|| {
                TakoError::Invalid(
                    "AbMcts: no rollout completed (try increasing max_iterations)".into(),
                )
            })?;
            let leaf = &nodes[leaf_idx];
            let final_message = leaf
                .output_message
                .clone()
                .unwrap_or_else(|| Message::assistant(""));
            let text = leaf.output_text.clone().unwrap_or_default();
            let total_steps = leaf.rollout_steps;
            yield OrchEvent::Final {
                output: Box::new(OrchOutput {
                    text,
                    message: final_message,
                    usage: total_usage,
                    steps: total_steps,
                }),
            };
        };

        Box::pin(s)
    }
}

struct IterationOutcome {
    leaf: usize,
    score: f32,
    rollout_usage: Usage,
}

impl AbMcts {
    /// One iteration of selection + (adaptive) expansion + simulation +
    /// verification + back-propagation.
    async fn iterate(
        &self,
        nodes: &mut Vec<Node>,
        rng: &mut StdRng,
        principal: &Principal,
        prompt_text: &str,
    ) -> Result<IterationOutcome, TakoError> {
        // ---- Selection / adaptive branching ----
        let mut current = 0usize; // root
        let mut path: Vec<usize> = vec![current];
        loop {
            let node = &nodes[current];
            // The "expand new sibling" candidate is a fresh Beta(1, 1)
            // sample. If it beats every existing child AND we haven't
            // hit branching_factor at this node, we expand.
            let new_child_sample = sample_beta(rng, 1.0, 1.0);
            let mut best_child: Option<usize> = None;
            let mut best_child_sample = f32::NEG_INFINITY;
            for &c in &node.children {
                let c_node = &nodes[c];
                let s = sample_beta(rng, c_node.alpha, c_node.beta);
                if s > best_child_sample {
                    best_child_sample = s;
                    best_child = Some(c);
                }
            }

            let can_branch = (node.children.len() as u32) < self.branching_factor;
            let should_expand = match best_child {
                None => true, // no children yet → always expand
                Some(_) => can_branch && new_child_sample > best_child_sample,
            };

            if should_expand {
                break; // expand a new child of `current`
            }
            // Descend into the best existing child. We only get here
            // when `best_child` is `Some`, since `should_expand` is
            // forced to `true` when there are no children.
            let Some(next) = best_child else { break };
            path.push(next);
            current = next;
        }

        // ---- Expansion + simulation ----
        let parent_idx = current;
        let parent_messages = nodes[parent_idx].messages.clone();
        let (rollout_message, rollout_text, rollout_usage, rollout_steps) =
            self.rollout(principal, parent_messages.clone()).await?;

        let mut child_messages = parent_messages;
        child_messages.push(rollout_message.clone());

        // ---- Verification ----
        let score = self
            .verifier
            .score(principal, prompt_text, &rollout_text)
            .await?
            .clamp(0.0, 1.0);

        // ---- Insert leaf ----
        let leaf = Node {
            messages: child_messages,
            alpha: 1.0,
            beta: 1.0,
            children: Vec::new(),
            output_text: Some(rollout_text),
            output_message: Some(rollout_message),
            rollout_steps,
        };
        let leaf_idx = nodes.len();
        nodes.push(leaf);
        nodes[parent_idx].children.push(leaf_idx);
        path.push(leaf_idx);

        // ---- Back-propagation ----
        for &n in &path {
            nodes[n].alpha += score;
            nodes[n].beta += 1.0 - score;
        }

        Ok(IterationOutcome {
            leaf: leaf_idx,
            score,
            rollout_usage,
        })
    }

    /// One rollout: extend the conversation `messages` for up to
    /// `max_steps_per_rollout` provider turns, looping on tool calls.
    /// Returns the final assistant `Message`, its text, cumulative
    /// `Usage`, and the number of provider steps taken.
    async fn rollout(
        &self,
        principal: &Principal,
        messages: Vec<Message>,
    ) -> Result<(Message, String, Usage, u32), TakoError> {
        rollout_static(
            Arc::clone(&self.provider),
            Arc::clone(&self.tools),
            self.policy.clone(),
            self.defaults.clone(),
            self.max_steps_per_rollout,
            self.temperature,
            principal,
            messages,
        )
        .await
    }
}

/// Free-function form of [`AbMcts::rollout`]. Extracted in v0.9.0 so
/// [`AbMcts::stream`] (Phase 8.B) can drive rollouts from inside an
/// `async_stream::try_stream!` block without holding a `&self`
/// reference across the stream's `'static` lifetime.
#[allow(clippy::too_many_arguments)]
async fn rollout_static(
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    policy: Option<Arc<dyn PolicyEngine>>,
    defaults: ChatDefaults,
    max_steps_per_rollout: u32,
    temperature: f32,
    principal: &Principal,
    mut messages: Vec<Message>,
) -> Result<(Message, String, Usage, u32), TakoError> {
    let mut total_usage = Usage::default();
    let mut steps = 0u32;
    let mut last_assistant: Option<Message> = None;
    let model = provider.id().split(':').nth(1).unwrap_or("").to_string();

    for step in 0..max_steps_per_rollout {
        let req = ChatRequest {
            model: model.clone(),
            messages: messages.clone(),
            tools: tools.schemas().await,
            temperature: Some(temperature),
            max_tokens: defaults.max_tokens,
            stop: Vec::new(),
            stream: false,
            metadata: Default::default(),
        };
        let span = info_span!(
            "tako.provider.chat",
            "tako.provider.id" = %provider.id(),
            "tako.provider.model" = %model,
            "tako.orchestrator.step" = step,
        );
        let resp = provider.chat(principal, req).instrument(span).await?;
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
            last_assistant = Some(assistant);
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
                match engine.evaluate(principal, ctx).await? {
                    PolicyDecision::Deny { reason } => {
                        return Err(TakoError::PolicyDenied(format!("tool `{name}`: {reason}")));
                    }
                    PolicyDecision::RequireApproval { reason } => {
                        return Err(TakoError::PolicyDenied(format!(
                            "tool `{name}` requires approval: {reason}"
                        )));
                    }
                    _ => {}
                }
            }
            let result = match tools.invoke(principal, &name, args).await {
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

        if step + 1 == max_steps_per_rollout {
            last_assistant = Some(assistant);
        }
    }

    let final_message = last_assistant.unwrap_or_else(|| Message::assistant(""));
    let text = final_message
        .content
        .iter()
        .filter_map(ContentPart::as_text)
        .collect::<Vec<_>>()
        .join("");
    Ok((final_message, text, total_usage, steps))
}

/// Sample from `Beta(α, β)` via the ratio of two `Gamma(·, 1)`
/// samples. This is the textbook approach and avoids pulling in
/// `rand_distr`.
fn sample_beta(rng: &mut StdRng, alpha: f32, beta: f32) -> f32 {
    let x = sample_gamma(rng, alpha.max(1e-3));
    let y = sample_gamma(rng, beta.max(1e-3));
    let denom = x + y;
    if denom <= 0.0 {
        0.5
    } else {
        (x / denom).clamp(0.0, 1.0)
    }
}

/// Sample from `Gamma(k, 1)` via Marsaglia–Tsang (k >= 1) and a boost
/// (k < 1) per the standard reduction.
fn sample_gamma(rng: &mut StdRng, k: f32) -> f32 {
    if k < 1.0 {
        // Boost: G(k) = G(k+1) * U^(1/k)
        let u: f32 = rng.random_range(1e-9..1.0);
        return sample_gamma(rng, k + 1.0) * u.powf(1.0 / k);
    }
    let d = k - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        // Standard normal via Box-Muller.
        let u1: f32 = rng.random_range(1e-9..1.0);
        let u2: f32 = rng.random_range(0.0..1.0);
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        let v_cube_root = 1.0 + c * z;
        if v_cube_root <= 0.0 {
            continue;
        }
        let v = v_cube_root * v_cube_root * v_cube_root;
        let u: f32 = rng.random_range(1e-9..1.0);
        let z2 = z * z;
        if u < 1.0 - 0.0331 * z2 * z2 {
            return d * v;
        }
        if u.ln() < 0.5 * z2 + d * (1.0 - v + v.ln()) {
            return d * v;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn sample_beta_stays_in_unit_interval() {
        let mut rng = StdRng::from_rng(&mut rand::rng());
        for &(a, b) in &[(1.0_f32, 1.0_f32), (5.0, 1.0), (1.0, 5.0), (0.5, 0.5)] {
            for _ in 0..200 {
                let s = sample_beta(&mut rng, a, b);
                assert!((0.0..=1.0).contains(&s), "sample {s} out of [0,1]");
            }
        }
    }

    #[test]
    fn sample_beta_concentrates_near_mean() {
        // Beta(50, 5) has mean 50/(50+5) ≈ 0.909. Average over many
        // samples should be in the right ballpark.
        let mut rng = StdRng::from_rng(&mut rand::rng());
        let mut sum = 0.0_f32;
        let n = 1000;
        for _ in 0..n {
            sum += sample_beta(&mut rng, 50.0, 5.0);
        }
        let mean = sum / n as f32;
        assert!((0.85..0.96).contains(&mean), "mean was {mean}");
    }
}
