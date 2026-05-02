//! Azure Key Vault resolver (REST API).
//!
//! Reads secrets via Azure Key Vault's REST endpoint
//! (`GET /secrets/{name}?api-version=...`). Authentication is deferred to
//! the caller — supply a pre-resolved OAuth2 access token. We avoid pulling
//! the Azure SDK to keep dep weight small and to dodge the still-unstable
//! `azure_security_keyvault_secrets` crate.
//!
//! For local dev: `az account get-access-token --resource https://vault.azure.net`.
//! For workload identity / managed identity: any standard token-acquisition
//! flow.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use tako_core::TakoError;

use super::{SecretResolver, SecretString};

const DEFAULT_API_VERSION: &str = "7.4";

#[derive(Clone)]
pub struct AzureKeyVaultResolver {
    vault_url: String,
    api_version: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for AzureKeyVaultResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureKeyVaultResolver")
            .field("vault_url", &self.vault_url)
            .field("api_version", &self.api_version)
            .field("access_token", &"<redacted>")
            .finish()
    }
}

impl AzureKeyVaultResolver {
    /// Build a resolver. `vault_url` is the Key Vault DNS name, e.g.
    /// `https://my-vault.vault.azure.net`. `access_token` is a pre-resolved
    /// Azure AD bearer token scoped to `https://vault.azure.net/.default`.
    pub fn new(
        vault_url: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Result<Self, TakoError> {
        Self::with_api_version(vault_url, access_token, DEFAULT_API_VERSION)
    }

    pub fn with_api_version(
        vault_url: impl Into<String>,
        access_token: impl Into<String>,
        api_version: impl Into<String>,
    ) -> Result<Self, TakoError> {
        let access_token: String = access_token.into();
        let mut headers = HeaderMap::new();
        let auth = HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|e| TakoError::Invalid(format!("invalid Azure access token: {e}")))?;
        headers.insert(AUTHORIZATION, auth);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        Ok(Self {
            vault_url: vault_url.into(),
            api_version: api_version.into(),
            http,
        })
    }
}

#[derive(Deserialize)]
struct AkvSecretBundle {
    value: String,
}

#[async_trait]
impl SecretResolver for AzureKeyVaultResolver {
    async fn resolve(&self, key: &str) -> Result<SecretString, TakoError> {
        // Optional version: `secret-name#version-id`.
        let (name, version) = match key.split_once('#') {
            Some((n, v)) => (n, Some(v)),
            None => (key, None),
        };
        let path = match version {
            Some(v) => format!("/secrets/{name}/{v}"),
            None => format!("/secrets/{name}"),
        };
        let url = format!(
            "{}{}?api-version={}",
            self.vault_url.trim_end_matches('/'),
            path,
            self.api_version,
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
                "azure key vault secret `{key}` not found"
            )));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(TakoError::provider(
                "azure-key-vault",
                self.vault_url.clone(),
                format!("HTTP {status}: {body}"),
            ));
        }
        let bundle: AkvSecretBundle = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        Ok(SecretString::new(bundle.value))
    }
}
