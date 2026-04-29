//! HashiCorp Vault resolver (KV-v2 REST API).
//!
//! Reads secrets from Vault's KV-v2 engine via a single GET call. We avoid
//! pulling the `vaultrs` SDK to keep the dep tree small; KV-v2 reads are
//! one HTTP call against a documented schema.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use tako_core::TakoError;

use super::{SecretResolver, SecretString};

/// Resolver against Vault's KV-v2 secret engine.
///
/// Keys are interpreted as `"<path>#<json-pointer>"` (the JSON pointer is
/// optional and selects a sub-key from the secret). For example:
///
/// - `"secret/data/myapp"` returns the entire `data` object as a JSON
///   string.
/// - `"secret/data/myapp#api_key"` returns just the `api_key` value.
///
/// The path must include the KV-v2 mount's `data/` segment (Vault's REST
/// API requires it).
#[derive(Clone)]
pub struct VaultResolver {
    addr: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for VaultResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultResolver")
            .field("addr", &self.addr)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl VaultResolver {
    /// Build a resolver against the Vault server at `addr` (e.g.
    /// `https://vault.example:8200`) using the given token. The token is
    /// sent on every request as the `X-Vault-Token` header.
    pub fn new(addr: impl Into<String>, token: impl Into<String>) -> Result<Self, TakoError> {
        let addr = addr.into();
        let token = token.into();
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_str(&token)
            .map_err(|e| TakoError::Invalid(format!("invalid Vault token: {e}")))?;
        headers.insert("X-Vault-Token", value);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        Ok(Self { addr, http })
    }

    /// Convenience: build from `VAULT_ADDR` + `VAULT_TOKEN` env vars.
    pub fn from_env() -> Result<Self, TakoError> {
        let addr = std::env::var("VAULT_ADDR")
            .map_err(|_| TakoError::Invalid("VAULT_ADDR not set".into()))?;
        let token = std::env::var("VAULT_TOKEN")
            .map_err(|_| TakoError::Invalid("VAULT_TOKEN not set".into()))?;
        Self::new(addr, token)
    }
}

#[derive(Deserialize)]
struct VaultKvV2Read {
    data: VaultKvV2Data,
}

#[derive(Deserialize)]
struct VaultKvV2Data {
    data: serde_json::Value,
}

#[async_trait]
impl SecretResolver for VaultResolver {
    async fn resolve(&self, key: &str) -> Result<SecretString, TakoError> {
        // Split off an optional JSON-pointer sub-selector after `#`.
        let (path, sub_key) = match key.split_once('#') {
            Some((p, s)) => (p, Some(s)),
            None => (key, None),
        };
        let url = format!(
            "{}/v1/{}",
            self.addr.trim_end_matches('/'),
            path.trim_start_matches('/'),
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(TakoError::NotFound(format!("vault secret `{key}` not found")));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(TakoError::provider(
                "vault",
                self.addr.clone(),
                format!("HTTP {status}: {body}"),
            ));
        }

        let body: VaultKvV2Read = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let value = match sub_key {
            None => serde_json::to_string(&body.data.data)
                .map_err(|e| TakoError::Invalid(format!("vault response not serializable: {e}")))?,
            Some(k) => match body.data.data.get(k) {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => {
                    return Err(TakoError::NotFound(format!(
                        "vault secret `{path}` has no key `{k}`"
                    )));
                }
            },
        };

        Ok(SecretString::new(value))
    }
}
