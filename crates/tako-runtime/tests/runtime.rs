//! `tako-runtime` integration tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use tako_core::{Budget, Principal, TakoError, Usage};
use tako_runtime::{
    BreakerConfig, BudgetTracker, CircuitBreaker, InMemoryBudgetBackend, ProviderLimiter, RetryConfig, current_principal,
    with_principal, with_retry,
};

#[tokio::test]
async fn principal_propagates_through_task_local() {
    let p = Principal::new("acme", "alice");
    let observed = with_principal(p.clone(), async move { current_principal() }).await;
    assert_eq!(observed, Some(p));
}

#[tokio::test]
async fn principal_outside_scope_is_none() {
    assert!(current_principal().is_none());
}

#[tokio::test]
async fn budget_pre_check_per_request_cap() {
    let budget = Budget {
        max_usd_per_request: Some(0.10),
        ..Default::default()
    };
    let t = BudgetTracker::in_memory(budget);
    let p = Principal::anonymous();
    assert!(t.pre_check(&p, 0.05, 100).await.is_ok());
    let err = t.pre_check(&p, 0.20, 100).await.unwrap_err();
    assert!(matches!(err, TakoError::BudgetExhausted(_)), "got {err:?}");
}

#[tokio::test]
async fn budget_per_day_accumulates() {
    let budget = Budget {
        max_usd_per_day: Some(1.00),
        ..Default::default()
    };
    let backend = Arc::new(InMemoryBudgetBackend::new());
    let t = BudgetTracker::new(backend.clone(), budget);
    let p = Principal::new("acme", "alice");
    t.record(&p, 0.80, Usage { input_tokens: 0, output_tokens: 0 })
        .await
        .unwrap();
    // 0.80 already + 0.30 estimate = 1.10 > 1.00 cap
    let err = t.pre_check(&p, 0.30, 0).await.unwrap_err();
    assert!(matches!(err, TakoError::BudgetExhausted(_)));
}

#[tokio::test]
async fn breaker_opens_after_threshold() {
    let bc = BreakerConfig {
        consecutive_failures: 3,
        min_cooldown: Duration::from_secs(2),
        max_cooldown: Duration::from_secs(2),
    };
    let cb = CircuitBreaker::new(bc);

    for _ in 0..3 {
        let r = cb
            .call(|| async { Err::<(), _>(TakoError::Transport("boom".into())) })
            .await;
        assert!(r.is_err());
    }

    // Next call should be rejected by the open breaker.
    let r = cb.call(|| async { Ok::<(), _>(()) }).await;
    assert!(matches!(r.unwrap_err(), TakoError::CircuitOpen));
}

#[tokio::test]
async fn retry_succeeds_on_transient_then_ok() {
    let attempts = AtomicU32::new(0);
    let cfg = RetryConfig {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(2),
        multiplier: 2.0,
        jitter: 0.0,
    };
    let result = with_retry(&cfg, |_attempt| async {
        let n = attempts.fetch_add(1, Ordering::SeqCst);
        if n < 2 {
            Err(TakoError::Transport("boom".into()))
        } else {
            Ok(42_u32)
        }
    })
    .await;
    assert_eq!(result.unwrap(), 42);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_does_not_retry_non_transient() {
    let attempts = AtomicU32::new(0);
    let cfg = RetryConfig {
        max_attempts: 5,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(2),
        multiplier: 2.0,
        jitter: 0.0,
    };
    let r: Result<(), TakoError> = with_retry(&cfg, |_| async {
        attempts.fetch_add(1, Ordering::SeqCst);
        Err(TakoError::Invalid("nope".into()))
    })
    .await;
    assert!(matches!(r.unwrap_err(), TakoError::Invalid(_)));
    assert_eq!(attempts.load(Ordering::SeqCst), 1, "non-transient error must NOT retry");
}

#[tokio::test]
async fn limiter_throttles_concurrent_acquires() {
    let limiter = ProviderLimiter::per_second(NonZeroU32::new(2).unwrap());
    let start = Instant::now();
    // First two acquires should be immediate (burst).
    limiter.acquire().await;
    limiter.acquire().await;
    // Third must wait ~500ms.
    limiter.acquire().await;
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300),
        "limiter let through too fast: {elapsed:?}"
    );
}
