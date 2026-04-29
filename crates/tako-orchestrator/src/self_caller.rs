//! `SelfCaller<O>`: bounded recursive wrapper around any other orchestrator.
//!
//! Generalisation of Sakana AI's *Fugu Beta* self-recursion pattern. After
//! the wrapped orchestrator emits an output, a [`ConfidenceGuard`] scores
//! it on `[0, 1]`. If the score is below `min_confidence` AND recursion
//! depth is below `max_depth`, the inner orchestrator is re-invoked with
//! a corrective prompt that includes the previous output.
//!
//! Depth is tracked via `Principal::metadata["tako.recursion.depth"]`
//! so accidental infinite loops are impossible — nested SelfCallers
//! across module boundaries still see the same depth counter.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
pub use tako_core::ConfidenceGuard;
use tako_core::{ChatRequest, FinishReason, LlmProvider, Message, Principal, TakoError};
use tako_runtime::BudgetTracker;
use tracing::{Instrument, info_span};

use crate::types::{OrchEvent, OrchInput, OrchOutput};
use crate::{Orchestrator, OrchestratorKind};

const DEPTH_KEY: &str = "tako.recursion.depth";
const DEFAULT_MAX_DEPTH: u8 = 3;
const DEFAULT_MIN_CONFIDENCE: f32 = 0.7;
const DEFAULT_REVISION_PROMPT: &str = "Your previous answer scored low on the \
    confidence guard. Read it carefully, identify any gaps or errors, and \
    produce a corrected response.";

/// Self-recursive wrapper over an `Arc<dyn Orchestrator>`. Trait-object form
/// so it can hold heterogeneous wrapped orchestrators (SingleAgent, Trinity,
/// Conductor) and remain dyn-compatible itself.
///
/// **Budget semantics**: `SelfCaller` itself has no provider call sites. All
/// regular execution flows through the wrapped `inner` orchestrator and is
/// metered through whatever budget that orchestrator was built with. The
/// only independent provider call inside this module is
/// [`LlmJudgeGuard::evaluate`]; its budget is configured separately via
/// [`LlmJudgeGuard::with_budget`].
pub struct SelfCaller {
    inner: Arc<dyn Orchestrator>,
    max_depth: u8,
    min_confidence: f32,
    confidence: Arc<dyn ConfidenceGuard>,
    revision_prompt: String,
}

impl std::fmt::Debug for SelfCaller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfCaller")
            .field("max_depth", &self.max_depth)
            .field("min_confidence", &self.min_confidence)
            .finish_non_exhaustive()
    }
}

impl SelfCaller {
    pub fn builder() -> SelfCallerBuilder {
        SelfCallerBuilder::default()
    }
}

#[derive(Default)]
pub struct SelfCallerBuilder {
    inner: Option<Arc<dyn Orchestrator>>,
    max_depth: Option<u8>,
    min_confidence: Option<f32>,
    confidence: Option<Arc<dyn ConfidenceGuard>>,
    revision_prompt: Option<String>,
}

impl std::fmt::Debug for SelfCallerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfCallerBuilder")
            .field("max_depth", &self.max_depth)
            .field("min_confidence", &self.min_confidence)
            .finish_non_exhaustive()
    }
}

impl SelfCallerBuilder {
    pub fn inner(mut self, o: Arc<dyn Orchestrator>) -> Self {
        self.inner = Some(o);
        self
    }
    pub fn max_depth(mut self, n: u8) -> Self {
        self.max_depth = Some(n);
        self
    }
    pub fn min_confidence(mut self, c: f32) -> Self {
        self.min_confidence = Some(c.clamp(0.0, 1.0));
        self
    }
    pub fn confidence(mut self, c: Arc<dyn ConfidenceGuard>) -> Self {
        self.confidence = Some(c);
        self
    }
    pub fn revision_prompt(mut self, p: impl Into<String>) -> Self {
        self.revision_prompt = Some(p.into());
        self
    }
    pub fn build(self) -> Result<SelfCaller, TakoError> {
        Ok(SelfCaller {
            inner: self
                .inner
                .ok_or_else(|| TakoError::Invalid("SelfCallerBuilder: inner is required".into()))?,
            max_depth: self.max_depth.unwrap_or(DEFAULT_MAX_DEPTH),
            min_confidence: self.min_confidence.unwrap_or(DEFAULT_MIN_CONFIDENCE),
            confidence: self.confidence.ok_or_else(|| {
                TakoError::Invalid("SelfCallerBuilder: confidence is required".into())
            })?,
            revision_prompt: self
                .revision_prompt
                .unwrap_or_else(|| DEFAULT_REVISION_PROMPT.to_string()),
        })
    }
}

fn read_depth(p: &Principal) -> u8 {
    p.metadata
        .get(DEPTH_KEY)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(0)
}

fn bumped_principal(p: &Principal, depth: u8) -> Principal {
    let mut new = p.clone();
    new.metadata
        .insert(DEPTH_KEY.to_string(), depth.to_string());
    new
}

#[async_trait]
impl Orchestrator for SelfCaller {
    fn kind(&self) -> OrchestratorKind {
        OrchestratorKind::SelfCaller
    }

    async fn run(&self, principal: &Principal, input: OrchInput) -> Result<OrchOutput, TakoError> {
        let span = info_span!(
            "tako.orchestrator.run",
            "tako.orchestrator.kind" = "self_caller",
            "tako.principal.tenant_id" = %principal.tenant_id,
            "tako.principal.user_id" = %principal.user_id,
        );
        async move {
            let starting_depth = read_depth(principal);
            let mut current_input = input;
            let mut last_output: Option<OrchOutput> = None;

            for offset in 0..=self.max_depth {
                let depth = starting_depth.saturating_add(offset);
                let p = bumped_principal(principal, depth);

                let span = info_span!(
                    "tako.recursion.step",
                    "tako.recursion.depth" = depth,
                    "tako.recursion.confidence" = tracing::field::Empty,
                );
                let out = self
                    .inner
                    .run(&p, current_input.clone())
                    .instrument(span.clone())
                    .await?;

                let conf = self.confidence.evaluate(&p, &out.text).await?;
                span.record("tako.recursion.confidence", conf);

                if conf >= self.min_confidence || offset >= self.max_depth {
                    return Ok(out);
                }

                // Below threshold; recurse with a corrective input.
                let mut next_messages = current_input.messages.clone();
                next_messages.push(Message::assistant(out.text.clone()));
                next_messages.push(Message::user(self.revision_prompt.clone()));
                current_input = OrchInput {
                    messages: next_messages,
                    system: current_input.system,
                };
                last_output = Some(out);
            }
            // Should be unreachable: the loop returns inside or via `>= max_depth`.
            last_output.ok_or_else(|| TakoError::Invalid("SelfCaller: no output produced".into()))
        }
        .instrument(span)
        .await
    }

    /// Streaming variant of [`SelfCaller::run`].
    ///
    /// Forwards every event from the inner orchestrator's stream
    /// verbatim, except [`OrchEvent::Final`] events on intermediate
    /// recursion iterations — those are captured and evaluated by the
    /// [`ConfidenceGuard`] instead of being yielded. Only the final
    /// accepted (or max-depth) iteration's `Final` event is forwarded.
    ///
    /// ## Recursion signal on the wire (v0.9.0)
    ///
    /// At the end of each recursion iteration (or at early-abort), the
    /// outer stream yields an [`OrchEvent::Recursion`] carrying the
    /// current depth and the guard's confidence score. Consumers can
    /// observe recursion progress without inferring it from
    /// `StepStart` ordering as in v0.8.0.
    ///
    /// ## Streaming-aware early termination (v0.9.0)
    ///
    /// After each [`OrchEvent::AssistantText`] delta forwarded from
    /// the inner stream, the guard's
    /// [`ConfidenceGuard::evaluate_streaming`] hook is consulted with
    /// the cumulative assistant text seen so far. If the hook returns
    /// `Some(score)` with `score >= self.min_confidence`, the inner
    /// stream is dropped, an [`OrchEvent::Recursion`] carrying the
    /// score is emitted, and a synthesised [`OrchEvent::Final`] over
    /// the accumulated text closes the stream — useful for cheap
    /// rule-based guards that can decide early.
    ///
    /// The default `evaluate_streaming` returns `Ok(None)`, so guards
    /// that don't override (incl. `LlmJudgeGuard`) are unaffected.
    async fn stream(
        &self,
        principal: &Principal,
        input: OrchInput,
    ) -> BoxStream<'static, Result<OrchEvent, TakoError>> {
        let inner = Arc::clone(&self.inner);
        let confidence = Arc::clone(&self.confidence);
        let max_depth = self.max_depth;
        let min_confidence = self.min_confidence;
        let revision_prompt = self.revision_prompt.clone();
        let principal = principal.clone();

        let s = async_stream::try_stream! {
            let starting_depth = read_depth(&principal);
            let mut current_input = input;
            let mut last_output: Option<OrchOutput> = None;

            for offset in 0..=max_depth {
                let depth = starting_depth.saturating_add(offset);
                let p = bumped_principal(&principal, depth);

                let span = info_span!(
                    "tako.recursion.step",
                    "tako.recursion.depth" = depth,
                    "tako.recursion.confidence" = tracing::field::Empty,
                );

                let mut inner_stream = inner
                    .stream(&p, current_input.clone())
                    .instrument(span.clone())
                    .await;
                let mut captured: Option<OrchOutput> = None;
                let mut accumulated = String::new();
                let mut early_score: Option<f32> = None;
                while let Some(ev) = inner_stream.next().await {
                    match ev? {
                        OrchEvent::Final { output } => {
                            captured = Some(*output);
                            // Do not forward intermediate Final events;
                            // the outer stream emits exactly one Final
                            // (the accepted iteration's) at the end.
                        }
                        OrchEvent::AssistantText { step, delta } => {
                            accumulated.push_str(&delta);
                            yield OrchEvent::AssistantText { step, delta };
                            // Phase 8.D: streaming-aware early-abort.
                            // Guards that override `evaluate_streaming`
                            // (e.g. `RuleBasedGuard`) can short-circuit
                            // before the inner orchestrator finishes.
                            // The default impl returns Ok(None), so
                            // guards that don't override are unaffected.
                            if let Some(score) =
                                confidence.evaluate_streaming(&p, &accumulated).await?
                            {
                                if score >= min_confidence {
                                    early_score = Some(score);
                                    break;
                                }
                            }
                        }
                        other => {
                            yield other;
                        }
                    }
                }

                if let Some(score) = early_score {
                    span.record("tako.recursion.confidence", score);
                    yield OrchEvent::Recursion { depth: depth as u32, confidence: score };
                    let out = OrchOutput {
                        text: accumulated.clone(),
                        message: Message::assistant(accumulated),
                        usage: tako_core::Usage::default(),
                        steps: 1,
                    };
                    yield OrchEvent::Final { output: Box::new(out) };
                    return;
                }

                let out = captured.ok_or_else(|| {
                    TakoError::Invalid(
                        "SelfCaller: inner stream ended without Final event".into(),
                    )
                })?;

                let conf = confidence.evaluate(&p, &out.text).await?;
                span.record("tako.recursion.confidence", conf);

                yield OrchEvent::Recursion { depth: depth as u32, confidence: conf };

                if conf >= min_confidence || offset >= max_depth {
                    yield OrchEvent::Final { output: Box::new(out) };
                    return;
                }

                let mut next_messages = current_input.messages.clone();
                next_messages.push(Message::assistant(out.text.clone()));
                next_messages.push(Message::user(revision_prompt.clone()));
                current_input = OrchInput {
                    messages: next_messages,
                    system: current_input.system,
                };
                last_output = Some(out);
            }
            // Should be unreachable: the loop yields-and-returns above
            // when offset >= max_depth.
            if let Some(out) = last_output {
                yield OrchEvent::Final { output: Box::new(out) };
            } else {
                Err(TakoError::Invalid("SelfCaller: stream produced no output".into()))?;
            }
        };

        Box::pin(s)
    }
}

// ---------------------------------------------------------------------------
// Concrete `ConfidenceGuard` implementations
// ---------------------------------------------------------------------------

/// Rule-based guard: returns `1.0` iff the output's text is at least
/// `min_chars` characters long and matches an optional regex; otherwise
/// `0.0`. Cheap default for "is this long enough to count as a real
/// answer?" checks.
#[derive(Debug)]
pub struct RuleBasedGuard {
    min_chars: usize,
    pattern: Option<regex::Regex>,
}

impl RuleBasedGuard {
    pub fn new(min_chars: usize) -> Self {
        Self {
            min_chars,
            pattern: None,
        }
    }
    pub fn with_pattern(mut self, pat: regex::Regex) -> Self {
        self.pattern = Some(pat);
        self
    }
}

#[async_trait]
impl ConfidenceGuard for RuleBasedGuard {
    async fn evaluate(&self, _principal: &Principal, text: &str) -> Result<f32, TakoError> {
        if text.len() < self.min_chars {
            return Ok(0.0);
        }
        if let Some(re) = &self.pattern {
            return Ok(if re.is_match(text) { 1.0 } else { 0.0 });
        }
        Ok(1.0)
    }

    /// Streaming-aware variant (Phase 8.D): if the accumulated partial
    /// already passes both the length check and (when configured) the
    /// regex, return `Some(1.0)` — the partial output is sufficient and
    /// `SelfCaller::stream` should early-abort. Otherwise return `None`
    /// so the inner stream keeps running.
    async fn evaluate_streaming(
        &self,
        _principal: &Principal,
        partial: &str,
    ) -> Result<Option<f32>, TakoError> {
        if partial.len() < self.min_chars {
            return Ok(None);
        }
        if let Some(re) = &self.pattern
            && !re.is_match(partial)
        {
            return Ok(None);
        }
        Ok(Some(1.0))
    }
}

/// LLM-as-judge guard: ask another provider to score the output 0..1.
/// The judge prompt asks for a single decimal between 0 and 1. Anything
/// unparseable falls back to `0.5`.
pub struct LlmJudgeGuard {
    judge: Arc<dyn LlmProvider>,
    rubric: String,
    /// Optional budget tracker consulted before each judge call
    /// (`pre_check`) and after each call (`record`). When `None`, the
    /// judge call is unmetered. SelfCaller does **not** carry a budget
    /// itself: regular execution is metered through the inner
    /// orchestrator's own budget; this hook covers the judge's
    /// independent provider call.
    budget: Option<Arc<BudgetTracker>>,
}

impl std::fmt::Debug for LlmJudgeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmJudgeGuard")
            .field("judge", &self.judge.id())
            .finish()
    }
}

impl LlmJudgeGuard {
    pub fn new(judge: Arc<dyn LlmProvider>, rubric: impl Into<String>) -> Self {
        Self {
            judge,
            rubric: rubric.into(),
            budget: None,
        }
    }

    /// Attach a [`BudgetTracker`]. When set, `evaluate()` consults
    /// `pre_check` before the judge call and `record` after, using
    /// [`tako_core::LlmProvider::estimate_cost_usd`] for both estimates.
    /// `BudgetExhausted` propagates as a `TakoError`.
    pub fn with_budget(mut self, t: Arc<BudgetTracker>) -> Self {
        self.budget = Some(t);
        self
    }
}

#[async_trait]
impl ConfidenceGuard for LlmJudgeGuard {
    async fn evaluate(&self, principal: &Principal, text: &str) -> Result<f32, TakoError> {
        let prompt = format!(
            "{}\n\nThe candidate answer is:\n---\n{}\n---\n\nReply with ONLY a decimal between 0 \
             and 1 representing your confidence in the answer's quality. No other text.",
            self.rubric, text,
        );
        let model = self.judge.id().split(':').nth(1).unwrap_or("").to_string();
        let req = ChatRequest::new(model, vec![Message::user(prompt)]);
        let estimated_usd = self.judge.estimate_cost_usd(&req);
        if let Some(b) = self.budget.as_ref() {
            let est_tokens = req.max_tokens.unwrap_or(0);
            b.pre_check(principal, estimated_usd, est_tokens).await?;
        }
        let resp = self.judge.chat(principal, req).await?;
        if let Some(b) = self.budget.as_ref() {
            b.record(principal, estimated_usd, resp.usage).await?;
        }
        if !matches!(
            resp.finish_reason,
            FinishReason::Stop | FinishReason::Length | FinishReason::ToolCalls
        ) {
            return Ok(0.5);
        }
        let raw = resp
            .message
            .content
            .iter()
            .filter_map(tako_core::ContentPart::as_text)
            .collect::<Vec<_>>()
            .join("");
        Ok(parse_confidence(&raw).unwrap_or(0.5))
    }
}

fn parse_confidence(text: &str) -> Option<f32> {
    text.split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .filter(|s| !s.is_empty())
        .find_map(|s| s.parse::<f32>().ok())
        .map(|f| f.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[tokio::test]
    async fn rule_based_guard_threshold() {
        let g = RuleBasedGuard::new(10);
        let p = Principal::anonymous();
        assert_eq!(g.evaluate(&p, "short").await.unwrap(), 0.0);
        assert_eq!(g.evaluate(&p, "this is long enough").await.unwrap(), 1.0);
    }

    #[tokio::test]
    async fn rule_based_guard_with_pattern() {
        let g = RuleBasedGuard::new(0).with_pattern(regex::Regex::new(r"\bdone\b").unwrap());
        let p = Principal::anonymous();
        assert_eq!(g.evaluate(&p, "all done now").await.unwrap(), 1.0);
        assert_eq!(g.evaluate(&p, "in progress").await.unwrap(), 0.0);
    }

    #[test]
    fn parse_confidence_extracts_first_number() {
        assert_eq!(parse_confidence("0.83"), Some(0.83));
        assert_eq!(parse_confidence("score: 0.42"), Some(0.42));
        assert_eq!(parse_confidence("nothing here"), None);
        // Out-of-range gets clamped.
        assert_eq!(parse_confidence("1.5"), Some(1.0));
        assert_eq!(parse_confidence("-0.2"), Some(0.0));
    }

    #[tokio::test]
    async fn read_depth_defaults_to_zero() {
        let p = Principal::anonymous();
        assert_eq!(read_depth(&p), 0);
    }

    #[tokio::test]
    async fn bumped_principal_increments_metadata() {
        let p = Principal::anonymous();
        let p2 = bumped_principal(&p, 1);
        assert_eq!(read_depth(&p2), 1);
        let p3 = bumped_principal(&p2, 2);
        assert_eq!(read_depth(&p3), 2);
    }
}
