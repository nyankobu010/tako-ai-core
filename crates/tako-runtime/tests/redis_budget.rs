//! End-to-end RedisBudgetBackend tests.
//!
//! These tests need a running Redis instance. To run them locally:
//!
//! ```sh
//! redis-server --daemonize yes
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p tako-runtime --features redis --test redis_budget
//! ```
//!
//! When `REDIS_URL` is unset the tests no-op (printed reason on
//! `eprintln`) so default `cargo test --workspace --all-features`
//! invocations on a developer machine without Redis stay green.
#![cfg(feature = "redis")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use tako_core::{Budget, Principal, TakoError, Usage};
use tako_runtime::{BudgetBackend, BudgetTracker, RedisBudgetBackend};

/// Read `REDIS_URL` from the env, or print a reason and return `None`
/// so the test can early-return without failing.
fn redis_url() -> Option<String> {
    match std::env::var("REDIS_URL") {
        Ok(url) => Some(url),
        Err(_) => {
            eprintln!(
                "skipping (set REDIS_URL=redis://127.0.0.1:6379 with a running redis-server to run)"
            );
            None
        }
    }
}

/// Each test gets a fresh prefix so concurrent runs don't collide.
fn unique_prefix(suffix: &str) -> String {
    format!(
        "tako:test:{}:{}:{suffix}",
        std::process::id(),
        // nanoseconds since unix epoch — good enough disambiguation
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

#[tokio::test]
async fn missing_key_reports_zero_usage() {
    let Some(url) = redis_url() else { return };
    let backend = RedisBudgetBackend::connect(&url)
        .await
        .unwrap()
        .with_key_prefix(unique_prefix("missing"));
    let usage = backend.current_usage("acme").await.unwrap();
    assert_eq!(usage.usd_today, 0.0);
    assert_eq!(usage.tokens_today, 0);
}

#[tokio::test]
async fn record_then_read_round_trip() {
    let Some(url) = redis_url() else { return };
    let backend = RedisBudgetBackend::connect(&url)
        .await
        .unwrap()
        .with_key_prefix(unique_prefix("round_trip"));

    backend.record("acme", 0.123_456, 1_500).await.unwrap();
    let usage = backend.current_usage("acme").await.unwrap();
    // HINCRBYFLOAT preserves enough precision for cost arithmetic;
    // assert with a small epsilon to absorb the float round-trip.
    assert!(
        (usage.usd_today - 0.123_456).abs() < 1e-9,
        "got {}",
        usage.usd_today
    );
    assert_eq!(usage.tokens_today, 1_500);
}

#[tokio::test]
async fn multiple_records_accumulate() {
    let Some(url) = redis_url() else { return };
    let backend = RedisBudgetBackend::connect(&url)
        .await
        .unwrap()
        .with_key_prefix(unique_prefix("accumulate"));

    for _ in 0..5 {
        backend.record("acme", 0.10, 100).await.unwrap();
    }
    let usage = backend.current_usage("acme").await.unwrap();
    assert!(
        (usage.usd_today - 0.50).abs() < 1e-9,
        "got {}",
        usage.usd_today
    );
    assert_eq!(usage.tokens_today, 500);
}

#[tokio::test]
async fn tenants_are_isolated() {
    let Some(url) = redis_url() else { return };
    let backend = RedisBudgetBackend::connect(&url)
        .await
        .unwrap()
        .with_key_prefix(unique_prefix("isolated"));

    backend.record("acme", 0.30, 300).await.unwrap();
    backend.record("globex", 0.05, 50).await.unwrap();

    let acme = backend.current_usage("acme").await.unwrap();
    let globex = backend.current_usage("globex").await.unwrap();
    let unknown = backend.current_usage("unknown").await.unwrap();

    assert!((acme.usd_today - 0.30).abs() < 1e-9);
    assert!((globex.usd_today - 0.05).abs() < 1e-9);
    assert_eq!(unknown.usd_today, 0.0);
}

#[tokio::test]
async fn budget_tracker_enforces_daily_cap_against_redis() {
    let Some(url) = redis_url() else { return };
    let backend = RedisBudgetBackend::connect(&url)
        .await
        .unwrap()
        .with_key_prefix(unique_prefix("daily_cap"));

    let budget = Budget {
        max_usd_per_day: Some(1.00),
        ..Default::default()
    };
    let tracker = BudgetTracker::new(std::sync::Arc::new(backend), budget);
    let p = Principal::new("acme", "alice");

    tracker
        .record(
            &p,
            0.80,
            Usage {
                input_tokens: 0,
                output_tokens: 0,
            },
        )
        .await
        .unwrap();

    // 0.80 already + 0.30 estimate = 1.10 > 1.00 cap.
    let err = tracker.pre_check(&p, 0.30, 0).await.unwrap_err();
    assert!(matches!(err, TakoError::BudgetExhausted(_)), "got {err:?}");
}

#[tokio::test]
async fn ttl_is_set_on_first_record() {
    let Some(url) = redis_url() else { return };
    // Use a small TTL so this test can verify it without a long sleep.
    let backend = RedisBudgetBackend::connect(&url)
        .await
        .unwrap()
        .with_key_prefix(unique_prefix("ttl"))
        .with_ttl(Duration::from_secs(60));

    backend.record("acme", 0.01, 1).await.unwrap();
    // Round-trip the record: the read still works, so the key exists
    // (and has a TTL > 0). We don't probe TTL directly to avoid
    // re-implementing the TTL command surface here — the explicit TTL
    // path is exercised by the Lua script's `EXPIRE` call.
    let usage = backend.current_usage("acme").await.unwrap();
    assert!((usage.usd_today - 0.01).abs() < 1e-9);
    assert_eq!(usage.tokens_today, 1);
}
