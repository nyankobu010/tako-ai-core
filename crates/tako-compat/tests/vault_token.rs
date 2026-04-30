//! Phase 15.B.1 — `VaultTokenProvider` integration tests against
//! wiremock-mocked Vault auth endpoints. No live Vault required.
#![cfg(feature = "vault")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use serde_json::json;
use tako_compat::{
    AppRoleTokenProvider, KubernetesTokenProvider, StaticVaultToken, VaultTokenProvider,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn static_token_returns_fixed_value_no_lease() {
    let p = StaticVaultToken::new("dev-static");
    let (t, ttl) = p.token().await.unwrap();
    assert_eq!(t, "dev-static");
    assert!(ttl.is_none());

    // Stable across calls.
    let (t2, _) = p.token().await.unwrap();
    assert_eq!(t2, "dev-static");
}

#[tokio::test]
async fn approle_login_parses_response_and_caches() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/approle/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": {
                "client_token": "s.abc123",
                "lease_duration": 3600,
            }
        })))
        .expect(1) // Cached after first call.
        .mount(&server)
        .await;

    let p = AppRoleTokenProvider::new(server.uri(), "role-id", "secret-id").unwrap();
    let (t, ttl) = p.token().await.unwrap();
    assert_eq!(t, "s.abc123");
    let lease = ttl.expect("lease present");
    // Lease seconds came from the response.
    assert!(lease.as_secs() >= 3590, "lease was {lease:?}");

    // Second call within TTL must be served from the cache (no second
    // POST fired, asserted by `.expect(1)` on the mock).
    let (t2, _) = p.token().await.unwrap();
    assert_eq!(t2, "s.abc123");
}

#[tokio::test]
async fn approle_login_propagates_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/approle/login"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let p = AppRoleTokenProvider::new(server.uri(), "r", "s").unwrap();
    let err = p.token().await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("500"), "expected 500 in error, got: {msg}");
}

#[tokio::test]
async fn kubernetes_login_uses_jwt_from_file() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/kubernetes/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": {
                "client_token": "s.k8s-token",
                "lease_duration": 600,
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Write a fake SA JWT to a tempfile.
    let dir = tempfile::tempdir().unwrap();
    let jwt_path = dir.path().join("token");
    tokio::fs::write(&jwt_path, "fake.jwt.payload\n")
        .await
        .unwrap();

    let p = KubernetesTokenProvider::new(server.uri(), "tako-role", &jwt_path).unwrap();
    let (t, _) = p.token().await.unwrap();
    assert_eq!(t, "s.k8s-token");

    // Cached.
    let (t2, _) = p.token().await.unwrap();
    assert_eq!(t2, "s.k8s-token");
}

#[tokio::test]
async fn kubernetes_jwt_missing_path_surfaces_transport_error() {
    let p = KubernetesTokenProvider::new("http://127.0.0.1:1", "role", "/nonexistent/path/jwt-xxx")
        .unwrap();
    let err = p.token().await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("kubernetes JWT path"), "got: {msg}");
    assert!(msg.contains("/nonexistent/path/jwt-xxx"), "got: {msg}");
}

#[tokio::test]
async fn approle_reauth_when_lease_expired() {
    let server = MockServer::start().await;
    // 1-second lease — quickly stale.
    Mock::given(method("POST"))
        .and(path("/v1/auth/approle/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "auth": {
                "client_token": "s.first",
                "lease_duration": 1,
            }
        })))
        .expect(2) // Two real requests are expected.
        .mount(&server)
        .await;

    let p = AppRoleTokenProvider::new(server.uri(), "r", "s").unwrap();
    let (t1, _) = p.token().await.unwrap();
    assert_eq!(t1, "s.first");

    // Wait past 0.9 * 1s. Use real-time sleep since `Instant` doesn't
    // honour `tokio::time::pause`.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let (t2, _) = p.token().await.unwrap();
    assert_eq!(t2, "s.first");
    // wiremock will assert .expect(2) on drop.
}
