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
#[async_trait]
pub trait Verifier: Send + Sync + 'static {
    async fn score(
        &self,
        principal: &Principal,
        prompt: &str,
        output: &str,
    ) -> Result<f32, TakoError>;
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
