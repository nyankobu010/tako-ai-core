//! `VaultAuthResolver` — resolves bearer tokens via HashiCorp Vault KV v2.
//!
//! Phase 14.B. Each incoming bearer token is looked up at
//! `<mount>/data/<path_prefix>/<token>` (default mount `secret`,
//! default prefix `tako/tokens`). The KV entry shape is:
//!
//! ```json
//! { "tenant_id": "acme", "user_id": "alice", "roles": ["admin"] }
//! ```
//!
//! Successful lookups are cached in-process for `cache_ttl` (default
//! 60s) to avoid hammering Vault on each request. Failed lookups are
//! NOT cached (no negative-cache amplification of typos / probes).
//!
//! Phase 15.B.1 — the Vault bearer token used to talk to Vault itself
//! is now produced by a pluggable [`VaultTokenProvider`]:
//! [`StaticVaultToken`] (default; matches v0.15.0 behaviour),
//! [`AppRoleTokenProvider`], or [`KubernetesTokenProvider`]. The
//! resolver builds (and caches) one `VaultClient` per distinct Vault
//! token it observes; on rotation, the new token simply produces a
//! new entry in the small bounded cache.
//!
//! NOTE: the **principal cache** (token → Principal) and the **Vault-
//! client cache** (Vault-token → `VaultClient`) are orthogonal — Vault
//! token rotation does not invalidate principal lookups.
//!
//! Phase 16.B.1 — Vault Enterprise multi-tenant deployments scope
//! every API call to a named **namespace** via the `X-Vault-Namespace`
//! HTTP header. [`VaultAuthResolver::with_namespace`] sets the
//! namespace once at builder time; it propagates to every
//! `VaultClient` built by [`VaultAuthResolver::get_or_build_client`].
//! `None` (the default) preserves OSS-Vault behaviour byte-for-byte.
//!
//! Errors map to `TakoError::Invalid("vault: ...")` for unknown
//! tokens / malformed entries (so [`crate::routes::resolve_principal`]
//! returns 401) and to `TakoError::Transport("vault: ...")` for
//! Vault network / 5xx errors.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use tako_core::{Principal, TakoError};
use tokio::sync::RwLock;
use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};

use super::AuthResolver;
use super::vault_token::{
    AppRoleTokenProvider, KubernetesTokenProvider, StaticVaultToken, VaultTokenProvider,
};

const DEFAULT_MOUNT: &str = "secret";
const DEFAULT_PATH_PREFIX: &str = "tako/tokens";
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(60);
/// Bounded `VaultClient` cache size — token rotation depth in practice.
const CLIENT_CACHE_LIMIT: usize = 4;

#[derive(Debug, Deserialize)]
struct VaultEntry {
    tenant_id: String,
    user_id: String,
    #[serde(default)]
    roles: Vec<String>,
}

pub struct VaultAuthResolver {
    addr: String,
    provider: Arc<dyn VaultTokenProvider>,
    /// Bounded LRU of `VaultClient`s keyed on Vault-token-string. We
    /// only ever build a fresh `VaultClient` when the underlying
    /// `VaultTokenProvider` returns a token we haven't seen before.
    client_cache: Arc<RwLock<HashMap<String, Arc<VaultClient>>>>,
    mount: String,
    path_prefix: String,
    cache_ttl: Duration,
    cache: Arc<RwLock<HashMap<String, (Principal, Instant)>>>,
    /// Phase 16.B.1 — Vault Enterprise namespace. `None` ⇒ OSS Vault
    /// (no `X-Vault-Namespace` header). Set via
    /// [`VaultAuthResolver::with_namespace`].
    namespace: Option<String>,
}

impl std::fmt::Debug for VaultAuthResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultAuthResolver")
            .field("addr", &self.addr)
            .field("mount", &self.mount)
            .field("path_prefix", &self.path_prefix)
            .field("cache_ttl", &self.cache_ttl)
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

impl VaultAuthResolver {
    /// Connect to Vault at `addr` (e.g. `http://127.0.0.1:8200`)
    /// using a fixed Vault token. Equivalent to v0.15.0 — internally
    /// constructs a [`StaticVaultToken`] provider so the rest of the
    /// resolver flow is identical to the rotating variants.
    pub fn new(addr: &str, vault_token: &str) -> Result<Self, TakoError> {
        Ok(Self::with_provider(
            addr,
            Arc::new(StaticVaultToken::new(vault_token)),
        ))
    }

    /// Connect to Vault at `addr` with a custom token provider —
    /// e.g. one that periodically re-authenticates via AppRole /
    /// Kubernetes auth methods.
    pub fn with_provider(addr: impl Into<String>, provider: Arc<dyn VaultTokenProvider>) -> Self {
        Self {
            addr: addr.into(),
            provider,
            client_cache: Arc::new(RwLock::new(HashMap::new())),
            mount: DEFAULT_MOUNT.into(),
            path_prefix: DEFAULT_PATH_PREFIX.into(),
            cache_ttl: DEFAULT_CACHE_TTL,
            cache: Arc::new(RwLock::new(HashMap::new())),
            namespace: None,
        }
    }

    /// Convenience: AppRole-rotating Vault token. POSTs `{role_id,
    /// secret_id}` to `<addr>/v1/auth/approle/login` lazily on each
    /// request whose cached lease has expired.
    pub fn with_approle(
        addr: impl Into<String>,
        role_id: impl Into<String>,
        secret_id: impl Into<String>,
    ) -> Result<Self, TakoError> {
        let addr = addr.into();
        let provider = Arc::new(AppRoleTokenProvider::new(addr.clone(), role_id, secret_id)?);
        Ok(Self::with_provider(addr, provider))
    }

    /// Convenience: Kubernetes-auth rotating Vault token. Reads the
    /// SA JWT from `jwt_path` on each (re-)auth.
    pub fn with_kubernetes(
        addr: impl Into<String>,
        role: impl Into<String>,
        jwt_path: impl Into<PathBuf>,
    ) -> Result<Self, TakoError> {
        let addr = addr.into();
        let provider = Arc::new(KubernetesTokenProvider::new(addr.clone(), role, jwt_path)?);
        Ok(Self::with_provider(addr, provider))
    }

    /// Convenience: in-pod Kubernetes auth — `jwt_path` defaults to
    /// the standard `/var/run/secrets/kubernetes.io/serviceaccount/token`.
    pub fn with_kubernetes_in_pod(
        addr: impl Into<String>,
        role: impl Into<String>,
    ) -> Result<Self, TakoError> {
        let addr = addr.into();
        let provider = Arc::new(KubernetesTokenProvider::in_pod(addr.clone(), role)?);
        Ok(Self::with_provider(addr, provider))
    }

    pub fn with_mount(mut self, m: impl Into<String>) -> Self {
        self.mount = m.into();
        self
    }

    pub fn with_path_prefix(mut self, p: impl Into<String>) -> Self {
        self.path_prefix = p.into();
        self
    }

    pub fn with_cache_ttl(mut self, d: Duration) -> Self {
        self.cache_ttl = d;
        self
    }

    /// Phase 16.B.1 — set the Vault Enterprise namespace for every
    /// outgoing request. The value is propagated through
    /// [`VaultClientSettingsBuilder::namespace`] so each cached
    /// `VaultClient` sends the `X-Vault-Namespace` header on every
    /// KV lookup. `None` (the default) keeps OSS Vault behaviour.
    ///
    /// Chainable on top of any
    /// [`VaultAuthResolver::new`] /
    /// [`VaultAuthResolver::with_provider`] /
    /// [`VaultAuthResolver::with_approle`] /
    /// [`VaultAuthResolver::with_kubernetes`] /
    /// [`VaultAuthResolver::with_kubernetes_in_pod`] constructor —
    /// namespace is orthogonal to auth method.
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    fn path_for(&self, token: &str) -> String {
        format!("{}/{token}", self.path_prefix.trim_end_matches('/'))
    }

    /// Get-or-build a `VaultClient` for the given Vault token. Bounded
    /// at [`CLIENT_CACHE_LIMIT`]; on overflow, an arbitrary entry is
    /// evicted (rotation depth is small in practice — 1-2 generations
    /// of overlapping tokens during a re-auth).
    async fn get_or_build_client(&self, vault_token: &str) -> Result<Arc<VaultClient>, TakoError> {
        {
            let guard = self.client_cache.read().await;
            if let Some(c) = guard.get(vault_token) {
                return Ok(Arc::clone(c));
            }
        }
        let mut builder = VaultClientSettingsBuilder::default();
        builder.address(&self.addr).token(vault_token);
        if let Some(ns) = self.namespace.as_ref() {
            builder.namespace(Some(ns.clone()));
        }
        let settings = builder
            .build()
            .map_err(|e| TakoError::Invalid(format!("vault: invalid client settings: {e}")))?;
        let client = VaultClient::new(settings)
            .map_err(|e| TakoError::Transport(format!("vault: client connect: {e}")))?;
        let client = Arc::new(client);
        {
            let mut guard = self.client_cache.write().await;
            // Re-check under write-lock — another caller may have
            // built one concurrently.
            if let Some(existing) = guard.get(vault_token) {
                return Ok(Arc::clone(existing));
            }
            if guard.len() >= CLIENT_CACHE_LIMIT {
                // Bounded eviction — drop an arbitrary entry. The
                // active token is about to be inserted, so any
                // evicted entry is by definition a stale rotation.
                if let Some(k) = guard.keys().next().cloned() {
                    guard.remove(&k);
                }
            }
            guard.insert(vault_token.to_string(), Arc::clone(&client));
        }
        Ok(client)
    }
}

#[async_trait]
impl AuthResolver for VaultAuthResolver {
    async fn resolve(&self, token: &str) -> Result<Principal, TakoError> {
        // Positive cache hit?
        {
            let guard = self.cache.read().await;
            if let Some((p, when)) = guard.get(token)
                && when.elapsed() < self.cache_ttl
            {
                return Ok(p.clone());
            }
        }

        let (vault_token, _ttl) = self.provider.token().await?;
        let client = self.get_or_build_client(&vault_token).await?;

        let path = self.path_for(token);
        let entry: VaultEntry = vaultrs::kv2::read(client.as_ref(), &self.mount, &path)
            .await
            .map_err(|e| {
                // vaultrs surfaces 404s as `RestClientError`s — opaque
                // here, but the practical effect is the same: unknown
                // token → 401 via `TakoError::Invalid`.
                TakoError::Invalid(format!("vault: lookup `{path}` failed: {e}"))
            })?;

        let principal = Principal {
            tenant_id: entry.tenant_id,
            user_id: entry.user_id,
            roles: entry.roles,
            trace_id: None,
            metadata: Default::default(),
        };

        let mut guard = self.cache.write().await;
        guard.insert(token.to_string(), (principal.clone(), Instant::now()));
        Ok(principal)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    #[test]
    fn vault_resolver_is_send_sync() {
        assert_send_sync::<VaultAuthResolver>();
    }

    #[test]
    fn vault_resolver_constructs() {
        let auth = VaultAuthResolver::new("http://127.0.0.1:8200", "dev-token").unwrap();
        assert_eq!(auth.mount, "secret");
        assert_eq!(auth.path_prefix, "tako/tokens");
        assert_eq!(auth.cache_ttl, DEFAULT_CACHE_TTL);
    }

    #[test]
    fn vault_resolver_builder_overrides() {
        let auth = VaultAuthResolver::new("http://127.0.0.1:8200", "dev-token")
            .unwrap()
            .with_mount("kv")
            .with_path_prefix("api/keys")
            .with_cache_ttl(Duration::from_secs(5));
        assert_eq!(auth.mount, "kv");
        assert_eq!(auth.path_prefix, "api/keys");
        assert_eq!(auth.cache_ttl, Duration::from_secs(5));
    }

    #[test]
    fn vault_path_for_token_strips_trailing_slash() {
        let auth = VaultAuthResolver::new("http://127.0.0.1:8200", "x")
            .unwrap()
            .with_path_prefix("tako/tokens/");
        assert_eq!(auth.path_for("abc123"), "tako/tokens/abc123");
    }

    #[test]
    fn vault_resolver_with_approle_constructs() {
        let auth = VaultAuthResolver::with_approle("http://127.0.0.1:8200", "role-id", "secret-id")
            .unwrap();
        assert_eq!(auth.addr, "http://127.0.0.1:8200");
    }

    #[test]
    fn vault_resolver_with_kubernetes_constructs() {
        let auth = VaultAuthResolver::with_kubernetes(
            "http://127.0.0.1:8200",
            "tako-role",
            PathBuf::from("/tmp/sa-token"),
        )
        .unwrap();
        assert_eq!(auth.addr, "http://127.0.0.1:8200");
    }

    #[test]
    fn vault_resolver_with_kubernetes_in_pod_constructs() {
        let auth = VaultAuthResolver::with_kubernetes_in_pod("http://127.0.0.1:8200", "tako-role")
            .unwrap();
        assert_eq!(auth.addr, "http://127.0.0.1:8200");
    }

    #[test]
    fn vault_resolver_namespace_default_none() {
        let auth = VaultAuthResolver::new("http://127.0.0.1:8200", "dev-token").unwrap();
        assert!(auth.namespace.is_none());
    }

    #[test]
    fn vault_resolver_with_namespace_sets_value() {
        let auth = VaultAuthResolver::new("http://127.0.0.1:8200", "dev-token")
            .unwrap()
            .with_namespace("eng-team");
        assert_eq!(auth.namespace.as_deref(), Some("eng-team"));
    }

    #[test]
    fn vault_resolver_with_namespace_chainable_with_other_builders() {
        // Phase 16.B.1 — namespace is orthogonal to mount / prefix /
        // ttl and to the underlying auth-method constructor.
        let auth = VaultAuthResolver::with_approle("http://127.0.0.1:8200", "role-id", "secret-id")
            .unwrap()
            .with_mount("kv")
            .with_path_prefix("api/keys")
            .with_namespace("acme")
            .with_cache_ttl(Duration::from_secs(5));
        assert_eq!(auth.mount, "kv");
        assert_eq!(auth.path_prefix, "api/keys");
        assert_eq!(auth.namespace.as_deref(), Some("acme"));
        assert_eq!(auth.cache_ttl, Duration::from_secs(5));
    }

    #[test]
    fn vault_resolver_namespace_appears_in_debug_repr() {
        let auth = VaultAuthResolver::new("http://127.0.0.1:8200", "dev-token")
            .unwrap()
            .with_namespace("eng-team");
        let dbg = format!("{auth:?}");
        assert!(
            dbg.contains("namespace") && dbg.contains("eng-team"),
            "namespace not in debug repr: {dbg}"
        );
    }
}
