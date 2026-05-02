//! `TakoError` semantics — display, `is_transient`, builder methods.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;
use tako_core::TakoError;

#[test]
fn provider_builder_attaches_status_and_body() {
    let err = TakoError::provider("openai:gpt-5", "gpt-5", "boom")
        .with_status(503)
        .with_raw_body(r#"{"error":"x"}"#);
    let TakoError::Provider { details, .. } = &err else {
        panic!("wrong variant: {err:?}");
    };
    assert_eq!(details.status_code, Some(503));
    assert_eq!(details.raw_body.as_deref(), Some(r#"{"error":"x"}"#));
}

#[test]
fn transient_classification() {
    assert!(TakoError::RateLimited(Duration::from_secs(1)).is_transient());
    assert!(TakoError::Timeout(Duration::from_secs(1)).is_transient());
    assert!(TakoError::Transport("conn reset".into()).is_transient());
    assert!(TakoError::CircuitOpen.is_transient());
    let p503 = TakoError::provider("p", "m", "boom").with_status(503);
    assert!(p503.is_transient());
    let p429 = TakoError::provider("p", "m", "boom").with_status(429);
    assert!(
        !p429.is_transient(),
        "429 is rate-limited; provider must surface RateLimited not Provider"
    );
    assert!(!TakoError::Invalid("nope".into()).is_transient());
    assert!(!TakoError::PolicyDenied("no".into()).is_transient());
    assert!(!TakoError::BudgetExhausted("over".into()).is_transient());
}

#[test]
fn display_format() {
    assert_eq!(TakoError::CircuitOpen.to_string(), "circuit open");
    assert_eq!(TakoError::Cancelled.to_string(), "cancelled");
    assert!(
        TakoError::Timeout(Duration::from_millis(500))
            .to_string()
            .contains("500")
    );
}
