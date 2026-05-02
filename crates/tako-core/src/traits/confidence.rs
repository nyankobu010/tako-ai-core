//! `ConfidenceGuard` — scores an orchestrator output on `[0.0, 1.0]`.
//!
//! Used by `SelfCaller` (Phase 3) to decide whether to spin up a corrective
//! workflow on the previous output. Implementations may be rule-based,
//! LLM-as-judge, or external scorers.

use async_trait::async_trait;

use crate::error::TakoError;
use crate::types::Principal;

/// Scores an orchestrator output. `1.0` = fully confident (do not recurse),
/// `0.0` = recurse if depth budget allows.
///
/// Implementations must be deterministic for the same `(principal, text)`
/// pair when used inside `SelfCaller` so that recursion termination is
/// predictable.
///
/// ## Streaming-aware early termination (v0.9.0)
///
/// Optionally override [`evaluate_streaming`](Self::evaluate_streaming)
/// to let `SelfCaller::stream` early-abort an in-progress generation
/// the moment a partial output reaches confidence — useful for cheap
/// rule-based heuristics where evaluating each delta is essentially
/// free. The default impl returns `Ok(None)` (skip — keep streaming
/// and evaluate the buffered final text), so impls that don't
/// override behave exactly as before.
///
/// **Cost note**: do not override `evaluate_streaming` for guards
/// that make a remote call (LLM-as-judge, network classifier, etc.) —
/// you would invoke the judge on every assistant-text delta. The
/// shipped [`super::super::traits::confidence::AlwaysConfident`] /
/// `ConstantConfidence` test fixtures and the
/// `tako_orchestrator::guards::LlmJudgeGuard` deliberately keep the
/// default `Ok(None)`.
#[async_trait]
pub trait ConfidenceGuard: Send + Sync + 'static {
    async fn evaluate(&self, principal: &Principal, text: &str) -> Result<f32, TakoError>;

    /// Streaming-aware variant: called by `SelfCaller::stream` after
    /// each `OrchEvent::AssistantText` delta with the *cumulative*
    /// assistant text accumulated so far for the current iteration.
    ///
    /// Return value:
    /// - `Ok(None)` — keep streaming; do not early-abort. Default.
    /// - `Ok(Some(score))` — propose a confidence score for the
    ///   partial output; `SelfCaller::stream` early-aborts if `score
    ///   >= self.min_confidence`.
    /// - `Err(_)` — fail the stream.
    ///
    /// The default impl returns `Ok(None)`. Override only when
    /// scoring partial text is essentially free (e.g. length / regex
    /// heuristics).
    async fn evaluate_streaming(
        &self,
        _principal: &Principal,
        _partial: &str,
    ) -> Result<Option<f32>, TakoError> {
        Ok(None)
    }
}

/// Always-confident guard (skips recursion). Useful as a default.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysConfident;

#[async_trait]
impl ConfidenceGuard for AlwaysConfident {
    async fn evaluate(&self, _principal: &Principal, _text: &str) -> Result<f32, TakoError> {
        Ok(1.0)
    }
}

/// Returns a fixed score regardless of input. Test fixture.
#[derive(Debug, Clone, Copy)]
pub struct ConstantConfidence(pub f32);

#[async_trait]
impl ConfidenceGuard for ConstantConfidence {
    async fn evaluate(&self, _principal: &Principal, _text: &str) -> Result<f32, TakoError> {
        Ok(self.0.clamp(0.0, 1.0))
    }
}
