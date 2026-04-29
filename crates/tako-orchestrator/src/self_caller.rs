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
use futures::stream::BoxStream;
pub use tako_core::ConfidenceGuard;
use tako_core::{ChatRequest, FinishReason, LlmProvider, Message, Principal, TakoError};
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

    async fn stream(
        &self,
        _principal: &Principal,
        _input: OrchInput,
    ) -> BoxStream<'static, Result<OrchEvent, TakoError>> {
        Box::pin(futures::stream::once(async move {
            Err(TakoError::Invalid(
                "SelfCaller streaming is Phase 4; use `run` for now".into(),
            ))
        }))
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
}

/// LLM-as-judge guard: ask another provider to score the output 0..1.
/// The judge prompt asks for a single decimal between 0 and 1. Anything
/// unparseable falls back to `0.5`.
pub struct LlmJudgeGuard {
    judge: Arc<dyn LlmProvider>,
    rubric: String,
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
        }
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
        let resp = self.judge.chat(principal, req).await?;
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
