//! Circuit breaker for provider calls. Wraps the `failsafe` crate so the
//! rest of `tako` interacts with a stable, async-friendly surface.

use std::sync::Arc;
use std::time::Duration;

use failsafe::Config;
use failsafe::backoff::EqualJittered;
use failsafe::failure_policy::ConsecutiveFailures;
use failsafe::futures::CircuitBreaker as FailsafeAsync;
use tako_core::TakoError;

/// Configuration knobs for the breaker.
#[derive(Clone, Debug)]
pub struct BreakerConfig {
    /// Open after this many consecutive failures.
    pub consecutive_failures: u32,
    /// Initial cool-down before half-open.
    pub min_cooldown: Duration,
    /// Maximum cool-down before half-open.
    pub max_cooldown: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            consecutive_failures: 5,
            min_cooldown: Duration::from_secs(10),
            max_cooldown: Duration::from_secs(60),
        }
    }
}

type BreakerInner = failsafe::StateMachine<ConsecutiveFailures<EqualJittered>, ()>;

/// Async circuit breaker. Cheaply cloneable.
#[derive(Clone)]
pub struct CircuitBreaker {
    inner: Arc<BreakerInner>,
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker").finish_non_exhaustive()
    }
}

impl CircuitBreaker {
    pub fn new(config: BreakerConfig) -> Self {
        // failsafe's equal_jittered backoff requires min >= 1s; saturate
        // smaller values to keep callers from panicking.
        let one_sec = Duration::from_secs(1);
        let min = config.min_cooldown.max(one_sec);
        let max = config.max_cooldown.max(min);
        let backoff = failsafe::backoff::equal_jittered(min, max);
        let policy =
            failsafe::failure_policy::consecutive_failures(config.consecutive_failures, backoff);
        let inner = Config::new().failure_policy(policy).build();
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Run an async operation through the breaker. If the breaker is open
    /// returns [`TakoError::CircuitOpen`]; otherwise propagates the
    /// operation's own error.
    pub async fn call<T, F, Fut>(&self, f: F) -> Result<T, TakoError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, TakoError>>,
    {
        let inner = Arc::clone(&self.inner);
        let fut = f();
        let result = FailsafeAsync::call(&*inner, fut).await;
        match result {
            Ok(v) => Ok(v),
            Err(failsafe::Error::Inner(e)) => Err(e),
            Err(failsafe::Error::Rejected) => Err(TakoError::CircuitOpen),
        }
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(BreakerConfig::default())
    }
}
