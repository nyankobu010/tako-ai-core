//! Rate limiter wrapping `governor`.

use std::num::NonZeroU32;
use std::sync::Arc;

use governor::clock::DefaultClock;
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

/// Per-provider rate limiter. Cheaply cloneable.
#[derive(Clone)]
pub struct ProviderLimiter {
    inner: Arc<Limiter>,
}

impl std::fmt::Debug for ProviderLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderLimiter").finish_non_exhaustive()
    }
}

impl ProviderLimiter {
    /// `requests_per_second` requests/second sustained, with a burst of 1.
    pub fn per_second(requests_per_second: NonZeroU32) -> Self {
        let quota = Quota::per_second(requests_per_second);
        let inner = Arc::new(RateLimiter::direct(quota));
        Self { inner }
    }

    /// Block until the limiter permits one request.
    pub async fn acquire(&self) {
        self.inner.until_ready().await;
    }
}
