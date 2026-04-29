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
    /// wiremock server, or for VPC-private endpoints.
    pub fn with_endpoint(
        project_id: impl Into<String>,
        access_token: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Result<Self, TakoError> {
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
            endpoint: endpoint.into(),
            http,
        })
    }
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
        // Key may be `<secret-name>` (default version `latest`) or
        // `<secret-name>#<version>`.
        let (secret, version) = match key.split_once('#') {
            Some((s, v)) => (s, v),
            None => (key, "latest"),
        };
        let url = format!(
            "{}/v1/projects/{}/secrets/{}/versions/{}:access",
            self.endpoint.trim_end_matches('/'),
            self.project_id,
            secret,
            version,
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
