//! `tako-runtime` — Tokio runtime helpers for tako.
//!
//! Budgets, circuit breakers, retries with jitter, rate limiters, fallback
//! provider chains, and `Principal` task-local propagation.

pub mod breaker;
pub mod budget;
pub mod fallback;
pub mod limiter;
pub mod principal;
pub mod retry;

pub use breaker::{BreakerConfig, CircuitBreaker};
pub use budget::{BudgetBackend, BudgetTracker, InMemoryBudgetBackend, TenantUsage};
pub use fallback::FallbackProvider;
pub use limiter::ProviderLimiter;
pub use principal::{current as current_principal, with_principal};
pub use retry::{RetryConfig, with_retry};
