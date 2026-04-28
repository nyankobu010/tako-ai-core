//! `Router` — Trinity-style learned routing.

use async_trait::async_trait;

use crate::error::TakoError;
use crate::types::{ChatRequest, Principal, RoutingDecision};

/// Picks one provider/model from a candidate pool. Implementations may be
/// rule-based (`RegexRouter`, `CostRouter`), an ONNX-loaded learned model,
/// or LLM-as-judge.
#[async_trait]
pub trait Router: Send + Sync + 'static {
    async fn route(
        &self,
        principal: &Principal,
        req: &ChatRequest,
        candidates: &[String],
    ) -> Result<RoutingDecision, TakoError>;
}
