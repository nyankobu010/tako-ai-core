//! Budget enforcement.
//!
//! Phase 1 ships an in-memory [`BudgetBackend`] keyed by tenant. The
//! `BudgetTracker` is consulted twice per request:
//!
//! 1. **Pre-check** before the provider call, using
//!    [`tako_core::LlmProvider::estimate_cost_usd`].
//! 2. **Reconcile** after the provider returns, using actual usage from
//!    [`tako_core::Usage`].
//!
//! Phase 4 will add a Redis-backed backend; the trait is the swap point.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tako_core::{Budget, Principal, TakoError, Usage};
use tokio::sync::Mutex;

/// Cumulative spend snapshot for a single tenant.
#[derive(Clone, Copy, Debug, Default)]
pub struct TenantUsage {
    pub usd_today: f64,
    pub tokens_today: u64,
}

#[async_trait]
pub trait BudgetBackend: Send + Sync + 'static + std::fmt::Debug {
    /// Cumulative usage for `tenant_id` since the start of the current day
    /// (UTC). Implementations are responsible for day rollover.
    async fn current_usage(&self, tenant_id: &str) -> Result<TenantUsage, TakoError>;

    /// Record incremental usage for `tenant_id`.
    async fn record(&self, tenant_id: &str, usd: f64, tokens: u64) -> Result<(), TakoError>;
}

/// In-memory backend. Day rollover is naive (the in-memory state is per
/// process and is reset only when [`InMemoryBudgetBackend::reset`] is
/// called); production deployments should use the Phase 4 Redis backend.
#[derive(Debug, Default)]
pub struct InMemoryBudgetBackend {
    inner: Mutex<HashMap<String, TenantUsage>>,
}

impl InMemoryBudgetBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test helper: clear all tracked usage.
    pub async fn reset(&self) {
        self.inner.lock().await.clear();
    }
}

#[async_trait]
impl BudgetBackend for InMemoryBudgetBackend {
    async fn current_usage(&self, tenant_id: &str) -> Result<TenantUsage, TakoError> {
        Ok(self
            .inner
            .lock()
            .await
            .get(tenant_id)
            .copied()
            .unwrap_or_default())
    }

    async fn record(&self, tenant_id: &str, usd: f64, tokens: u64) -> Result<(), TakoError> {
        let mut g = self.inner.lock().await;
        let entry = g.entry(tenant_id.to_string()).or_default();
        entry.usd_today += usd;
        entry.tokens_today = entry.tokens_today.saturating_add(tokens);
        Ok(())
    }
}

/// Per-request, per-tenant budget enforcer.
#[derive(Clone, Debug)]
pub struct BudgetTracker {
    backend: Arc<dyn BudgetBackend>,
    budget: Arc<Budget>,
}

impl BudgetTracker {
    pub fn new(backend: Arc<dyn BudgetBackend>, budget: Budget) -> Self {
        Self {
            backend,
            budget: Arc::new(budget),
        }
    }

    /// Test convenience: in-memory tracker.
    pub fn in_memory(budget: Budget) -> Self {
        Self::new(Arc::new(InMemoryBudgetBackend::new()), budget)
    }

    /// Check whether an estimated request fits within the budget. Errors
    /// with [`TakoError::BudgetExhausted`] when limits are exceeded.
    pub async fn pre_check(
        &self,
        principal: &Principal,
        estimated_usd: f64,
        estimated_tokens: u32,
    ) -> Result<(), TakoError> {
        if let Some(max) = self.budget.max_usd_per_request {
            if estimated_usd > max {
                return Err(TakoError::BudgetExhausted(format!(
                    "request estimated at ${estimated_usd:.4} exceeds per-request cap ${max:.4}"
                )));
            }
        }
        if let Some(max) = self.budget.max_tokens_per_request {
            if estimated_tokens > max {
                return Err(TakoError::BudgetExhausted(format!(
                    "request estimated at {estimated_tokens} tokens exceeds per-request cap {max}"
                )));
            }
        }
        let cur = self.backend.current_usage(&principal.tenant_id).await?;
        if let Some(max) = self.budget.max_usd_per_day {
            if cur.usd_today + estimated_usd > max {
                return Err(TakoError::BudgetExhausted(format!(
                    "tenant `{}` would exceed daily cap ${max:.2}",
                    principal.tenant_id
                )));
            }
        }
        if let Some(max) = self
            .budget
            .max_usd_per_tenant_per_day
            .get(&principal.tenant_id)
        {
            if cur.usd_today + estimated_usd > *max {
                return Err(TakoError::BudgetExhausted(format!(
                    "tenant `{}` would exceed tenant-specific daily cap ${max:.2}",
                    principal.tenant_id
                )));
            }
        }
        Ok(())
    }

    /// Record actual cost after a provider call.
    pub async fn record(
        &self,
        principal: &Principal,
        usd: f64,
        usage: Usage,
    ) -> Result<(), TakoError> {
        self.backend
            .record(&principal.tenant_id, usd, u64::from(usage.total()))
            .await
    }
}
