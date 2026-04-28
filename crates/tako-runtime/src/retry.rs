//! Exponential-backoff retry with jitter, layered on top of the
//! [`tako_core::TakoError::is_transient`] classifier.

use std::time::Duration;

use rand::Rng;
use tako_core::TakoError;

#[derive(Clone, Debug)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub multiplier: f64,
    /// Random fraction of the backoff to add as jitter (0.0–1.0).
    pub jitter: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(20),
            multiplier: 2.0,
            jitter: 0.2,
        }
    }
}

/// Run an async operation with retry. Retries only on transient errors
/// (per [`TakoError::is_transient`]).
pub async fn with_retry<T, F, Fut>(config: &RetryConfig, mut f: F) -> Result<T, TakoError>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, TakoError>>,
{
    let mut delay = config.initial_backoff;
    let mut last_err: Option<TakoError> = None;
    for attempt in 0..config.max_attempts {
        match f(attempt).await {
            Ok(v) => return Ok(v),
            Err(e) if !e.is_transient() => return Err(e),
            Err(e) => {
                tracing::warn!(attempt, error = %e, "transient error; will retry");
                last_err = Some(e);
                if attempt + 1 >= config.max_attempts {
                    break;
                }
                let jitter_factor: f64 = {
                    let mut rng = rand::thread_rng();
                    1.0 + rng.gen_range(0.0..=config.jitter)
                };
                let sleep = delay.mul_f64(jitter_factor);
                tokio::time::sleep(sleep).await;
                let next_ms = (delay.as_millis() as f64 * config.multiplier) as u64;
                delay = Duration::from_millis(next_ms).min(config.max_backoff);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| TakoError::Invalid("retry loop exited without an error".into())))
}
