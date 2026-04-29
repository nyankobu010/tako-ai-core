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
#[async_trait]
pub trait ConfidenceGuard: Send + Sync + 'static {
    async fn evaluate(&self, principal: &Principal, text: &str) -> Result<f32, TakoError>;
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
