//! Phase 15.B.1 — Vault bearer-token providers.
//!
//! [`VaultAuthResolver`] (Phase 14.B) shipped with a static Vault
//! token baked in at construction. Production deployments need
//! periodic re-auth via AppRole or Kubernetes auth methods so the
//! resolver doesn't outlive its credential. This module abstracts the
//! token-acquisition strategy behind the [`VaultTokenProvider`] trait.
//!
//! Three impls ship:
//!
//! - [`StaticVaultToken`] — wraps a single string. Lossless equivalent
//!   of v0.15.0 behaviour; `VaultAuthResolver::new(addr, token)`
//!   internally constructs one of these.
//! - [`AppRoleTokenProvider`] — POSTs `{role_id, secret_id}` to
//!   `<addr>/v1/auth/approle/login`, parses `auth.client_token` +
//!   `auth.lease_duration`, re-authenticates lazily at
//!   `0.9 * lease_duration`.
//! - [`KubernetesTokenProvider`] — reads the SA JWT from a configurable
//!   path (typically `/var/run/secrets/kubernetes.io/serviceaccount/token`),
//!   POSTs `{role, jwt}` to `<addr>/v1/auth/kubernetes/login`. Same
//!   caching pattern as AppRole.
//!
//! All providers use `reqwest` directly (rather than vaultrs' auth
//! modules) so we don't bump the `vaultrs 0.7` dep.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tako_core::TakoError;
use tokio::sync::RwLock;

/// Refresh threshold: re-authenticate when the cached lease has been
/// alive for at least this fraction of its `lease_duration`. 0.9
/// matches the convention used by Vault's own agent (HCP 2024 docs).
const REFRESH_FRACTION: f32 = 0.9;

/// Default request timeout for Vault auth-login endpoints.
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Standard pod path for the Kubernetes service-account JWT. Matches
/// the `automountServiceAccountToken: true` default in K8s pods.
pub const DEFAULT_KUBERNETES_JWT_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";

/// Resolves a Vault bearer token, optionally refreshing it on each
/// call. Implementations must be `Send + Sync + 'static` so the
/// resolver can be shared across request threads.
#[async_trait]
pub trait VaultTokenProvider: Send + Sync + 'static + std::fmt::Debug {
    /// Returns the current Vault bearer token plus its remaining lease
    /// (if known). `None` lease means "no expiry hint" — e.g. static
    /// tokens. The resolver only re-fetches when the previous call's
    /// lease has expired.
    async fn token(&self) -> Result<(String, Option<Duration>), TakoError>;
}

// ---------------------------------------------------------------------------
// StaticVaultToken
// ---------------------------------------------------------------------------

/// Returns a fixed Vault token forever. Lossless equivalent of the
/// pre-Phase-15 `VaultAuthResolver::new(addr, vault_token)` behaviour.
#[derive(Debug, Clone)]
pub struct StaticVaultToken(String);

impl StaticVaultToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }
}

#[async_trait]
impl VaultTokenProvider for StaticVaultToken {
    async fn token(&self) -> Result<(String, Option<Duration>), TakoError> {
        Ok((self.0.clone(), None))
    }
}

// ---------------------------------------------------------------------------
// AppRoleTokenProvider
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VaultAuthResponse {
    auth: VaultAuthBody,
}

#[derive(Debug, Deserialize)]
struct VaultAuthBody {
    client_token: String,
    #[serde(default)]
    lease_duration: u64,
}

/// Cache slot: `(token, fetched_at, lease_duration)`. Re-fetch when
/// `fetched_at.elapsed() >= lease_duration * REFRESH_FRACTION`.
#[derive(Debug, Clone)]
struct CachedAuth {
    token: String,
    fetched_at: Instant,
    lease_duration: Duration,
}

/// Authenticates against Vault's AppRole auth method.
///
/// Each `token()` call checks the in-process cache first; if the
/// cached token is older than `0.9 * lease_duration`, a fresh
/// AppRole login is performed lazily. Static `role_id` / `secret_id`
/// are held in memory; rotating these is out of scope for this
/// provider.
#[derive(Debug)]
pub struct AppRoleTokenProvider {
    addr: String,
    role_id: String,
    secret_id: String,
    http: Client,
    cache: Arc<RwLock<Option<CachedAuth>>>,
}

impl AppRoleTokenProvider {
    pub fn new(
        addr: impl Into<String>,
        role_id: impl Into<String>,
        secret_id: impl Into<String>,
    ) -> Result<Self, TakoError> {
        let http = Client::builder()
            .timeout(DEFAULT_HTTP_TIMEOUT)
            .build()
            .map_err(|e| TakoError::Transport(format!("vault: approle: build http client: {e}")))?;
        Ok(Self {
            addr: addr.into(),
            role_id: role_id.into(),
            secret_id: secret_id.into(),
            http,
            cache: Arc::new(RwLock::new(None)),
        })
    }
}

#[async_trait]
impl VaultTokenProvider for AppRoleTokenProvider {
    async fn token(&self) -> Result<(String, Option<Duration>), TakoError> {
        // Fast path: positive cache, still fresh.
        {
            let guard = self.cache.read().await;
            if let Some(entry) = guard.as_ref()
                && is_fresh(entry)
            {
                let remaining = entry
                    .lease_duration
                    .saturating_sub(entry.fetched_at.elapsed());
                return Ok((entry.token.clone(), Some(remaining)));
            }
        }

        // Re-authenticate.
        let url = format!("{}/v1/auth/approle/login", self.addr.trim_end_matches('/'));
        let body = json!({
            "role_id": self.role_id,
            "secret_id": self.secret_id,
        });
        let resp = vault_login(&self.http, &url, &body).await?;
        let lease = Duration::from_secs(resp.auth.lease_duration.max(1));
        let entry = CachedAuth {
            token: resp.auth.client_token.clone(),
            fetched_at: Instant::now(),
            lease_duration: lease,
        };
        {
            let mut guard = self.cache.write().await;
            *guard = Some(entry);
        }
        Ok((resp.auth.client_token, Some(lease)))
    }
}

// ---------------------------------------------------------------------------
// KubernetesTokenProvider
// ---------------------------------------------------------------------------

/// Authenticates against Vault's Kubernetes auth method.
///
/// On each `token()` call, the SA JWT is read fresh from `jwt_path`
/// (so SA-token rotation is picked up), then POSTed alongside `role`
/// to `<addr>/v1/auth/kubernetes/login`. Cached lease handling
/// matches [`AppRoleTokenProvider`].
///
/// The constructor is infallible; missing-JWT errors surface only when
/// `token()` is called, so unit tests on developer workstations can
/// construct the provider without a populated `/var/run/secrets/...`.
#[derive(Debug)]
pub struct KubernetesTokenProvider {
    addr: String,
    role: String,
    jwt_path: PathBuf,
    http: Client,
    cache: Arc<RwLock<Option<CachedAuth>>>,
}

impl KubernetesTokenProvider {
    /// Construct a provider that reads the SA JWT from `jwt_path` on
    /// each (re-)auth.
    pub fn new(
        addr: impl Into<String>,
        role: impl Into<String>,
        jwt_path: impl Into<PathBuf>,
    ) -> Result<Self, TakoError> {
        let http = Client::builder()
            .timeout(DEFAULT_HTTP_TIMEOUT)
            .build()
            .map_err(|e| {
                TakoError::Transport(format!("vault: kubernetes: build http client: {e}"))
            })?;
        Ok(Self {
            addr: addr.into(),
            role: role.into(),
            jwt_path: jwt_path.into(),
            http,
            cache: Arc::new(RwLock::new(None)),
        })
    }

    /// Convenience constructor for in-pod deployments. Hardcodes
    /// `jwt_path` to [`DEFAULT_KUBERNETES_JWT_PATH`].
    pub fn in_pod(addr: impl Into<String>, role: impl Into<String>) -> Result<Self, TakoError> {
        Self::new(addr, role, PathBuf::from(DEFAULT_KUBERNETES_JWT_PATH))
    }
}

#[async_trait]
impl VaultTokenProvider for KubernetesTokenProvider {
    async fn token(&self) -> Result<(String, Option<Duration>), TakoError> {
        {
            let guard = self.cache.read().await;
            if let Some(entry) = guard.as_ref()
                && is_fresh(entry)
            {
                let remaining = entry
                    .lease_duration
                    .saturating_sub(entry.fetched_at.elapsed());
                return Ok((entry.token.clone(), Some(remaining)));
            }
        }

        let jwt = tokio::fs::read_to_string(&self.jwt_path)
            .await
            .map_err(|e| {
                TakoError::Transport(format!(
                    "vault: kubernetes JWT path `{}` unreadable: {e}",
                    self.jwt_path.display(),
                ))
            })?;
        let url = format!(
            "{}/v1/auth/kubernetes/login",
            self.addr.trim_end_matches('/'),
        );
        let body = json!({
            "role": self.role,
            "jwt": jwt.trim(),
        });
        let resp = vault_login(&self.http, &url, &body).await?;
        let lease = Duration::from_secs(resp.auth.lease_duration.max(1));
        let entry = CachedAuth {
            token: resp.auth.client_token.clone(),
            fetched_at: Instant::now(),
            lease_duration: lease,
        };
        {
            let mut guard = self.cache.write().await;
            *guard = Some(entry);
        }
        Ok((resp.auth.client_token, Some(lease)))
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn is_fresh(entry: &CachedAuth) -> bool {
    let elapsed = entry.fetched_at.elapsed().as_secs_f32();
    let cap = entry.lease_duration.as_secs_f32() * REFRESH_FRACTION;
    elapsed < cap
}

async fn vault_login(
    http: &Client,
    url: &str,
    body: &serde_json::Value,
) -> Result<VaultAuthResponse, TakoError> {
    let resp = http
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| TakoError::Transport(format!("vault: login `{url}`: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(TakoError::Invalid(format!(
            "vault: login `{url}` returned {status}: {body_text}"
        )));
    }
    resp.json::<VaultAuthResponse>()
        .await
        .map_err(|e| TakoError::Invalid(format!("vault: login response parse: {e}")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    #[test]
    fn vault_token_provider_impls_are_send_sync() {
        assert_send_sync::<StaticVaultToken>();
        assert_send_sync::<AppRoleTokenProvider>();
        assert_send_sync::<KubernetesTokenProvider>();
    }

    #[tokio::test]
    async fn static_vault_token_returns_fixed_value() {
        let p = StaticVaultToken::new("dev-token");
        let (t, ttl) = p.token().await.unwrap();
        assert_eq!(t, "dev-token");
        assert!(ttl.is_none());
    }

    #[tokio::test]
    async fn approle_constructor_does_not_authenticate() {
        // Constructor is infallible and does no I/O — even an
        // unreachable Vault address is fine.
        let p = AppRoleTokenProvider::new("http://127.0.0.1:1", "role-id", "secret-id").unwrap();
        // The cache starts empty.
        assert!(p.cache.read().await.is_none());
    }

    #[tokio::test]
    async fn kubernetes_jwt_missing_path_surfaces_transport_error() {
        let p = KubernetesTokenProvider::new(
            "http://127.0.0.1:1",
            "role",
            PathBuf::from("/nonexistent/path/jwt"),
        )
        .unwrap();
        let err = p.token().await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("kubernetes JWT path"),
            "expected JWT-path error, got: {msg}",
        );
        assert!(msg.contains("/nonexistent/path/jwt"));
    }
}
