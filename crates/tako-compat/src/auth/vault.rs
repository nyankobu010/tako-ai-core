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
//! Out-of-scope (deferred to Phase 15+): Vault token rotation
//! (AppRole, Kubernetes auth methods); the resolver uses the static
//! Vault token passed at construction. Operators wanting periodic
//! re-auth must rebuild the resolver themselves.
//!
//! Errors map to `TakoError::Invalid("vault: ...")` for unknown
//! tokens / malformed entries (so [`crate::routes::resolve_principal`]
//! returns 401) and to `TakoError::Transport("vault: ...")` for
//! Vault network / 5xx errors.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use tako_core::{Principal, TakoError};
use tokio::sync::RwLock;
use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};

use super::AuthResolver;

const DEFAULT_MOUNT: &str = "secret";
const DEFAULT_PATH_PREFIX: &str = "tako/tokens";
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
struct VaultEntry {
    tenant_id: String,
    user_id: String,
    #[serde(default)]
    roles: Vec<String>,
}

pub struct VaultAuthResolver {
    client: Arc<VaultClient>,
    mount: String,
    path_prefix: String,
    cache_ttl: Duration,
    cache: Arc<RwLock<HashMap<String, (Principal, Instant)>>>,
}

impl std::fmt::Debug for VaultAuthResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultAuthResolver")
            .field("mount", &self.mount)
            .field("path_prefix", &self.path_prefix)
            .field("cache_ttl", &self.cache_ttl)
            .finish_non_exhaustive()
    }
}

impl VaultAuthResolver {
    /// Connect to Vault at `addr` (e.g. `http://127.0.0.1:8200`)
    /// using the supplied root or service Vault token. Vault token
    /// rotation is out of scope — see module docs.
    pub fn new(addr: &str, vault_token: &str) -> Result<Self, TakoError> {
        let settings = VaultClientSettingsBuilder::default()
            .address(addr)
            .token(vault_token)
            .build()
            .map_err(|e| TakoError::Invalid(format!("vault: invalid client settings: {e}")))?;
        let client = VaultClient::new(settings)
            .map_err(|e| TakoError::Transport(format!("vault: client connect: {e}")))?;
        Ok(Self {
            client: Arc::new(client),
            mount: DEFAULT_MOUNT.into(),
            path_prefix: DEFAULT_PATH_PREFIX.into(),
            cache_ttl: DEFAULT_CACHE_TTL,
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
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

    fn path_for(&self, token: &str) -> String {
        format!("{}/{token}", self.path_prefix.trim_end_matches('/'))
    }
}

#[async_trait]
impl AuthResolver for VaultAuthResolver {
    async fn resolve(&self, token: &str) -> Result<Principal, TakoError> {
        // Positive cache hit?
        {
            let guard = self.cache.read().await;
            if let Some((p, when)) = guard.get(token) {
                if when.elapsed() < self.cache_ttl {
                    return Ok(p.clone());
                }
            }
        }

        let path = self.path_for(token);
        let entry: VaultEntry = vaultrs::kv2::read(self.client.as_ref(), &self.mount, &path)
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
}
