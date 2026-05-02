//! `Verifier` — scores a finished rollout on `[0.0, 1.0]`.
//!
//! Used by AB-MCTS (Phase 4) and any future tree-search orchestrator
//! to back-propagate a reward signal. The contract is identical to
//! `ConfidenceGuard` semantically (both score on `[0, 1]`) but the
//! input differs: a `Verifier` sees a complete rollout (prompt +
//! produced text), whereas a `ConfidenceGuard` sees only the output
//! text. Keeping the two traits separate avoids overloading the
//! semantics — a guard is "is this answer good enough to return?",
//! a verifier is "what reward should this leaf contribute to its
//! parent posterior?".

use async_trait::async_trait;

use crate::error::TakoError;
use crate::types::Principal;

/// Scores a complete rollout on `[0.0, 1.0]`. `1.0` = perfect,
/// `0.0` = useless. Used by AB-MCTS to update Beta posteriors during
/// back-propagation.
///
/// Implementations should be deterministic for the same
/// `(principal, prompt, output)` triple so that tree search converges.
///
/// ## Streaming-aware partial scoring (Phase 13.B, v0.14.0)
///
/// Optionally override [`evaluate_streaming`](Self::evaluate_streaming)
/// to let an orchestrator emit per-delta `OrchEvent::VerifierScore`
/// events on cumulative partial outputs — useful for cheap
/// rule-based heuristics where evaluating each delta is essentially
/// free (regex pass-rate, length-based scoring). The default impl
/// returns `Ok(None)` (skip — the orchestrator only emits the
/// authoritative synthesis-complete `score` at the end), so existing
/// impls behave exactly as before.
///
/// **Cost note**: do not override `evaluate_streaming` for verifiers
/// that make a remote call (LLM-as-judge, network classifier, etc.) —
/// you would invoke the verifier on every assistant-text delta. The
/// shipped [`AlwaysScore`] test fixture deliberately keeps the
/// default `Ok(None)`.
///
/// **Event shape.** The same `OrchEvent::VerifierScore { step,
/// branch, score }` event variant carries both partial and final
/// scores. Consumers distinguish by `(step, branch)` repetition:
/// multiple emissions on the same `(step, branch)` are streaming
/// partials; the last emission for a given `(step, branch)` is the
/// authoritative synthesis-complete score.
#[async_trait]
pub trait Verifier: Send + Sync + 'static {
    async fn score(
        &self,
        principal: &Principal,
        prompt: &str,
        output: &str,
    ) -> Result<f32, TakoError>;

    /// Streaming-aware variant: called by streaming orchestrators
    /// (`Trinity::stream` in Phase 13.B) after each
    /// `OrchEvent::AssistantText` delta with the *cumulative*
    /// assistant text accumulated so far for the current step.
    ///
    /// Return value:
    /// - `Ok(None)` — skip; do not emit a partial `VerifierScore`.
    ///   Default.
    /// - `Ok(Some(score))` — orchestrator emits an
    ///   `OrchEvent::VerifierScore` on the same `(step, branch)` as
    ///   the eventual synthesis-complete final.
    /// - `Err(_)` — fail the stream (same semantics as `score`).
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

/// Returns a fixed score regardless of input. Test fixture.
#[derive(Debug, Clone, Copy)]
pub struct AlwaysScore(pub f32);

#[async_trait]
impl Verifier for AlwaysScore {
    async fn score(
        &self,
        _principal: &Principal,
        _prompt: &str,
        _output: &str,
    ) -> Result<f32, TakoError> {
        Ok(self.0.clamp(0.0, 1.0))
    }
}
