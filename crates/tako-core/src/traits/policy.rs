//! `PolicyEngine` — pre-completion / pre-tool / post-completion enforcement.

use async_trait::async_trait;

use crate::error::TakoError;
use crate::types::{PolicyContext, PolicyDecision, Principal};

/// Pre-completion / pre-tool / post-completion policy enforcement. The
/// default Phase-2 implementation in `tako-governance` wraps regorus (OPA),
/// but any rule engine can implement this.
#[async_trait]
pub trait PolicyEngine: Send + Sync + 'static {
    async fn evaluate(&self, principal: &Principal, ctx: PolicyContext) -> Result<PolicyDecision, TakoError>;
}

/// `AllowAll` — a no-op policy useful as a default and for tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAll;

#[async_trait]
impl PolicyEngine for AllowAll {
    async fn evaluate(&self, _principal: &Principal, _ctx: PolicyContext) -> Result<PolicyDecision, TakoError> {
        Ok(PolicyDecision::Allow)
    }
}
