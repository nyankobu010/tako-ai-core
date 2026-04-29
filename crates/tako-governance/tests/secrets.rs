//! End-to-end tests for cloud secret resolvers (Vault, Azure KV, GCP SM)
//! against wiremock servers. AWS Secrets Manager is exercised through
//! its SDK + endpoint_url override at the unit level — full LocalStack-
//! backed coverage is out of scope.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use tako_core::TakoError;
use tako_governance::{
    AzureKeyVaultResolver, GcpSecretManagerResolver, SecretResolver, VaultResolver,
};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn vault_kv_v2_reads_full_secret_object() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "data": {
            "data": {
                "api_key": "sk-test-123",
                "endpoint": "https://upstream.example",
            }
        }
    });
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/myapp"))
        .and(header("X-Vault-Token", "vault-root-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let resolver = VaultResolver::new(server.uri(), "vault-root-token").unwrap();
    let secret = resolver.resolve("secret/data/myapp").await.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(secret.expose()).unwrap();
    assert_eq!(parsed["api_key"], "sk-test-123");
}

#[tokio::test]
async fn vault_kv_v2_reads_sub_key_via_jsonpointer() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "data": {
            "data": {"api_key": "sk-test-123"}
        }
    });
    Mock::given(method("GET"))
        .and(path("/v1/secret/data/myapp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let resolver = VaultResolver::new(server.uri(), "tok").unwrap();
    let secret = resolver
        .resolve("secret/data/myapp#api_key")
        .await
        .unwrap();
    assert_eq!(secret.expose(), "sk-test-123");
}

#[tokio::test]
async fn vault_404_maps_to_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let resolver = VaultResolver::new(server.uri(), "tok").unwrap();
    let err = resolver.resolve("secret/data/missing").await.unwrap_err();
    assert!(matches!(err, TakoError::NotFound(_)));
}

#[tokio::test]
async fn vault_missing_subkey_maps_to_not_found() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "data": {"data": {"foo": "bar"}}
    });
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let resolver = VaultResolver::new(server.uri(), "tok").unwrap();
    let err = resolver
        .resolve("secret/data/myapp#missing")
        .await
        .unwrap_err();
    assert!(matches!(err, TakoError::NotFound(_)));
}

#[tokio::test]
async fn azure_key_vault_reads_secret() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "value": "p@ssw0rd",
        "id": "https://x.vault.azure.net/secrets/db-password/abc",
    });
    Mock::given(method("GET"))
        .and(path("/secrets/db-password"))
        .and(query_param("api-version", "7.4"))
        .and(header("Authorization", "Bearer azure-token-xyz"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let resolver = AzureKeyVaultResolver::new(server.uri(), "azure-token-xyz").unwrap();
    let secret = resolver.resolve("db-password").await.unwrap();
    assert_eq!(secret.expose(), "p@ssw0rd");
}

#[tokio::test]
async fn azure_key_vault_with_version() {
    let server = MockServer::start().await;
    let body = serde_json::json!({"value": "old-value"});
    Mock::given(method("GET"))
        .and(path("/secrets/db-password/v123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let resolver = AzureKeyVaultResolver::new(server.uri(), "tok").unwrap();
    let secret = resolver.resolve("db-password#v123").await.unwrap();
    assert_eq!(secret.expose(), "old-value");
}

#[tokio::test]
async fn gcp_secret_manager_reads_latest() {
    let server = MockServer::start().await;
    let plaintext = "gcp-secret-value";
    let encoded = B64.encode(plaintext.as_bytes());
    let body = serde_json::json!({
        "name": "projects/p/secrets/s/versions/1",
        "payload": {"data": encoded},
    });
    Mock::given(method("GET"))
        .and(path("/v1/projects/my-proj/secrets/api-key/versions/latest:access"))
        .and(header("Authorization", "Bearer gcp-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let resolver =
        GcpSecretManagerResolver::with_endpoint("my-proj", "gcp-token", server.uri()).unwrap();
    let secret = resolver.resolve("api-key").await.unwrap();
    assert_eq!(secret.expose(), plaintext);
}

#[tokio::test]
async fn gcp_secret_manager_with_specific_version() {
    let server = MockServer::start().await;
    let body = serde_json::json!({
        "payload": {"data": B64.encode(b"v3-value")}
    });
    Mock::given(method("GET"))
        .and(path("/v1/projects/p/secrets/s/versions/3:access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let resolver = GcpSecretManagerResolver::with_endpoint("p", "tok", server.uri()).unwrap();
    let secret = resolver.resolve("s#3").await.unwrap();
    assert_eq!(secret.expose(), "v3-value");
}

#[tokio::test]
async fn gcp_404_maps_to_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let resolver = GcpSecretManagerResolver::with_endpoint("p", "tok", server.uri()).unwrap();
    let err = resolver.resolve("missing").await.unwrap_err();
    assert!(matches!(err, TakoError::NotFound(_)));
}

#[tokio::test]
async fn aws_resolver_constructs_without_credentials() {
    use tako_governance::AwsSecretsManagerResolver;
    // AWS resolver defers credential chain resolution to first resolve()
    // call — must construct cleanly even with no AWS env vars set.
    let r = AwsSecretsManagerResolver::new()
        .with_region("us-west-2")
        .with_profile("nonexistent-profile")
        .with_endpoint_url("http://127.0.0.1:1");
    let _: String = format!("{:?}", r);
}
