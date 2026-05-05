//! GCP Secret Manager resolver (REST API).
//!
//! Reads via `:access` against the Secret Manager v1 API. Like Vertex and
//! Azure KV, authentication is deferred — caller supplies a pre-resolved
//! OAuth2 access token (e.g. from `gcloud auth print-access-token` or
//! the GKE metadata server).

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use tako_core::TakoError;

use super::{SecretResolver, SecretString};

#[derive(Clone)]
pub struct GcpSecretManagerResolver {
    project_id: String,
    endpoint: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for GcpSecretManagerResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcpSecretManagerResolver")
            .field("project_id", &self.project_id)
            .field("endpoint", &self.endpoint)
            .field("access_token", &"<redacted>")
            .finish()
    }
}

impl GcpSecretManagerResolver {
    /// Build a resolver for the given GCP project. `access_token` is a
    /// pre-resolved OAuth2 token scoped to
    /// `https://www.googleapis.com/auth/cloud-platform`.
    pub fn new(
        project_id: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Result<Self, TakoError> {
        Self::with_endpoint(
            project_id,
            access_token,
            "https://secretmanager.googleapis.com",
        )
    }

    /// Override the Secret Manager endpoint. Useful for tests against a
    /// wiremock server (loopback HTTP is allowed), or for VPC-private
    /// endpoints (which must still be HTTPS). The endpoint must use
    /// `https://`, except for loopback hosts (`127.0.0.1`, `[::1]`,
    /// `localhost`) where `http://` is permitted for local testing.
    pub fn with_endpoint(
        project_id: impl Into<String>,
        access_token: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Result<Self, TakoError> {
        let endpoint: String = endpoint.into();
        if !endpoint_uses_secure_transport(&endpoint) {
            return Err(TakoError::Invalid(format!(
                "GCP Secret Manager endpoint must use https:// (got `{endpoint}`)"
            )));
        }
        let access_token: String = access_token.into();
        let mut headers = HeaderMap::new();
        let auth = HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|e| TakoError::Invalid(format!("invalid GCP access token: {e}")))?;
        headers.insert(AUTHORIZATION, auth);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        Ok(Self {
            project_id: project_id.into(),
            endpoint,
            http,
        })
    }
}

/// Returns true when `endpoint` is safe to send a Bearer token over —
/// either HTTPS (production), or HTTP pointed at a loopback host
/// (test/dev only). Used to gate `GcpSecretManagerResolver` against the
/// `rust/cleartext-transmission` failure mode where a misconfigured
/// `http://` endpoint would expose the OAuth2 token in cleartext.
fn endpoint_uses_secure_transport(endpoint: &str) -> bool {
    let lc = endpoint.to_ascii_lowercase();
    if lc.starts_with("https://") {
        return true;
    }
    let Some(rest) = lc.strip_prefix("http://") else {
        return false;
    };
    // Extract the host. IPv6 literals are bracketed (`[::1]:8080`) and
    // contain colons, so a naive `find(':')` would truncate the host.
    let host = if rest.starts_with('[') {
        match rest.find(']') {
            Some(end) => &rest[..=end],
            None => return false,
        }
    } else {
        let host_end = rest.find([':', '/']).unwrap_or(rest.len());
        &rest[..host_end]
    };
    matches!(host, "127.0.0.1" | "[::1]" | "localhost")
}

#[derive(Deserialize)]
struct GcpAccessSecretVersionResponse {
    payload: GcpSecretPayload,
}

#[derive(Deserialize)]
struct GcpSecretPayload {
    /// Secret value, base64-encoded.
    data: String,
}

#[async_trait]
impl SecretResolver for GcpSecretManagerResolver {
    async fn resolve(&self, key: &str) -> Result<SecretString, TakoError> {
        // Defense in depth: re-validate that the endpoint enforces
        // encrypted transport before sending the Bearer token + secret
        // identifier. The constructor already rejects insecure
        // endpoints, but re-checking here keeps the sanitizer in the
        // same function as the `get(&url)` sink (so static analysers
        // can see the dataflow) and guards against any future change
        // that could let a non-HTTPS endpoint slip through.
        let endpoint = self.endpoint.trim_end_matches('/');
        if !endpoint_uses_secure_transport(endpoint) {
            return Err(TakoError::Invalid(format!(
                "GCP Secret Manager endpoint must use https:// (got `{}`)",
                self.endpoint
            )));
        }
        // Key may be `<secret-name>` (default version `latest`) or
        // `<secret-name>#<version>`. `secret_name` is the GCP Secret
        // Manager identifier (URL path segment), not the secret value —
        // the value comes back in the response body.
        let (secret_name, version) = match key.split_once('#') {
            Some((s, v)) => (s, v),
            None => (key, "latest"),
        };
        let url = format!(
            "{endpoint}/v1/projects/{}/secrets/{secret_name}/versions/{version}:access",
            self.project_id,
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(TakoError::NotFound(format!(
                "gcp secret manager secret `{key}` not found"
            )));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(TakoError::provider(
                "gcp-secret-manager",
                self.project_id.clone(),
                format!("HTTP {status}: {body}"),
            ));
        }
        let parsed: GcpAccessSecretVersionResponse = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        let raw = B64
            .decode(parsed.payload.data.as_bytes())
            .map_err(|e| TakoError::Invalid(format!("gcp secret payload not base64: {e}")))?;
        let value = String::from_utf8(raw)
            .map_err(|e| TakoError::Invalid(format!("gcp secret payload not UTF-8: {e}")))?;
        Ok(SecretString::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::endpoint_uses_secure_transport;

    #[test]
    fn https_endpoints_are_accepted() {
        assert!(endpoint_uses_secure_transport(
            "https://secretmanager.googleapis.com"
        ));
        assert!(endpoint_uses_secure_transport(
            "https://secretmanager.us-central1.rep.googleapis.com"
        ));
        assert!(endpoint_uses_secure_transport("https://example.com:8443/"));
        // Scheme is case-insensitive per RFC 3986.
        assert!(endpoint_uses_secure_transport(
            "HTTPS://secretmanager.googleapis.com"
        ));
    }

    #[test]
    fn http_loopback_endpoints_are_accepted() {
        assert!(endpoint_uses_secure_transport("http://127.0.0.1:54321"));
        assert!(endpoint_uses_secure_transport("http://127.0.0.1/"));
        assert!(endpoint_uses_secure_transport("http://127.0.0.1"));
        assert!(endpoint_uses_secure_transport("http://[::1]:8080"));
        assert!(endpoint_uses_secure_transport("http://[::1]/"));
        assert!(endpoint_uses_secure_transport("http://[::1]"));
        assert!(endpoint_uses_secure_transport("http://localhost:8080"));
        assert!(endpoint_uses_secure_transport("http://localhost/"));
        assert!(endpoint_uses_secure_transport("http://localhost"));
    }

    #[test]
    fn http_non_loopback_endpoints_are_rejected() {
        assert!(!endpoint_uses_secure_transport(
            "http://secretmanager.googleapis.com"
        ));
        assert!(!endpoint_uses_secure_transport("http://example.com"));
        assert!(!endpoint_uses_secure_transport("http://10.0.0.1:8080"));
        // Non-loopback IPv6 must not be confused with `[::1]`.
        assert!(!endpoint_uses_secure_transport("http://[::2]"));
        assert!(!endpoint_uses_secure_transport("http://[2001:db8::1]:8080"));
        // `localhost`-prefixed hostnames must not slip through as loopback.
        assert!(!endpoint_uses_secure_transport("http://localhost.evil.com"));
        assert!(!endpoint_uses_secure_transport("http://127.0.0.1.evil.com"));
        // Unterminated IPv6 bracket.
        assert!(!endpoint_uses_secure_transport("http://[::1"));
    }

    #[test]
    fn malformed_endpoints_are_rejected() {
        assert!(!endpoint_uses_secure_transport(""));
        assert!(!endpoint_uses_secure_transport(
            "secretmanager.googleapis.com"
        ));
        assert!(!endpoint_uses_secure_transport("ftp://example.com"));
        assert!(!endpoint_uses_secure_transport("http://"));
    }
}
