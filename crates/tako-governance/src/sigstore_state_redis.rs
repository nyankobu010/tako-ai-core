//! Redis-backed [`StateStore`] for the [`KeylessVerifier`] Rekor
//! checkpoint freshness anchor.
//!
//! Phase 10.A's [`JsonStateStore`](crate::sigstore_state::JsonStateStore)
//! is single-process: in a multi-replica deployment where multiple
//! workers share the same anchor, each replica's on-disk file is
//! independent and a slow replica can silently advance its local
//! water-mark below another replica's. `RedisStateStore` keeps a
//! single shared `tako:sigstore:rekor_min_tree_size` key in Redis.
//!
//! Cross-replica safety lives in a small Lua script that enforces a
//! **monotonic** write — the cross-process analogue of
//! [`KeylessVerifier::rekor_max_tree_size`](crate::sigstore::KeylessVerifier::rekor_max_tree_size)'s
//! in-process `fetch_max`:
//!
//! ```text
//! local cur = tonumber(redis.call('GET', KEYS[1])) or 0
//! if tonumber(ARGV[1]) >= cur then
//!     redis.call('SET', KEYS[1], ARGV[1])
//!     return ARGV[1]
//! else
//!     return tostring(cur)
//! end
//! ```
//!
//! `save(n)` returns `Ok(())` regardless of whether `n` won or lost
//! the compare — the next `load()` from any replica reads the
//! authoritative high-water mark. There is no TTL: unlike
//! [`crate::RedisBudgetBackend`]'s daily-bucketed spend, the Rekor
//! anchor is permanent state.
//!
//! Gated behind the `redis` cargo feature so the `redis` crate (and
//! its TLS / async-runtime infrastructure) only land in the dep tree
//! when explicitly enabled.

use async_trait::async_trait;
use redis::{AsyncCommands, Script, aio::ConnectionManager};
use tako_core::TakoError;

use crate::sigstore_state::StateStore;

const DEFAULT_KEY: &str = "tako:sigstore:rekor_min_tree_size";

/// Atomic monotonic write. KEYS[1]=anchor key, ARGV[1]=new value.
/// Returns the value the key holds *after* the script runs (either the
/// new value, if it won, or the existing higher value, if it lost).
const SAVE_LUA: &str = r#"
local raw = redis.call('GET', KEYS[1])
local cur = tonumber(raw) or 0
local new = tonumber(ARGV[1])
if new == nil then
    return redis.error_reply("rekor_min_tree_size: ARGV[1] not an integer")
end
if new >= cur then
    redis.call('SET', KEYS[1], ARGV[1])
    return ARGV[1]
else
    return tostring(cur)
end
"#;

/// Redis-backed [`StateStore`] for the Rekor checkpoint freshness
/// anchor in multi-replica deployments.
#[derive(Clone)]
pub struct RedisStateStore {
    manager: ConnectionManager,
    key: String,
    save_script: Script,
}

impl std::fmt::Debug for RedisStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisStateStore")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl RedisStateStore {
    /// Connect to `url` (e.g. `redis://localhost:6379` or
    /// `rediss://example.com:6379` for TLS) and prepare a multiplexed
    /// connection manager that auto-reconnects on transient failures.
    pub async fn connect(url: &str) -> Result<Self, TakoError> {
        let client = redis::Client::open(url)
            .map_err(|e| TakoError::Transport(format!("redis open: {e}")))?;
        let manager = ConnectionManager::new(client)
            .await
            .map_err(|e| TakoError::Transport(format!("redis connect: {e}")))?;
        Ok(Self {
            manager,
            key: DEFAULT_KEY.into(),
            save_script: Script::new(SAVE_LUA),
        })
    }

    /// Override the redis key (default `"tako:sigstore:rekor_min_tree_size"`).
    /// Useful for namespacing per environment or per anchor.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// The redis key backing this store. Useful for operators who
    /// manage backups / restores out-of-band.
    pub fn key(&self) -> &str {
        &self.key
    }
}

#[async_trait]
impl StateStore for RedisStateStore {
    async fn load(&self) -> Result<u64, TakoError> {
        let mut conn = self.manager.clone();
        let raw: Option<String> = conn
            .get(&self.key)
            .await
            .map_err(|e| TakoError::Transport(format!("redis get: {e}")))?;
        match raw {
            None => Ok(0),
            Some(s) => s.parse::<u64>().map_err(|e| {
                TakoError::Invalid(format!(
                    "sigstore_state_redis: parse {} = {s:?}: {e}",
                    self.key,
                ))
            }),
        }
    }

    async fn save(&self, n: u64) -> Result<(), TakoError> {
        let mut conn = self.manager.clone();
        // The script returns the post-write value (either `n` or a
        // higher pre-existing value). We don't need it for the
        // contract — the caller's high-water mark either won the
        // compare or was overtaken by a peer; either way `load()`
        // from any replica now reads the authoritative max. We use
        // `String` to absorb either the numeric reply or the redis
        // string reply uniformly.
        let _post: String = self
            .save_script
            .key(&self.key)
            .arg(n)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| TakoError::Transport(format!("redis eval: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Compile-time assertion: `RedisStateStore` implements the
    /// [`StateStore`] trait so callers can hold
    /// `Arc<dyn StateStore>` and pick the backend at runtime.
    #[test]
    fn redis_state_store_implements_state_store() {
        fn assert_state_store<T: StateStore>() {}
        assert_state_store::<RedisStateStore>();
    }

    /// Live-Redis integration tests. Opt-in via `cargo test
    /// --features redis -- --ignored` against
    /// `redis://localhost:6379`. Mirrors the
    /// [`crate::RedisBudgetBackend`] test gating.
    fn live_redis_url() -> Option<String> {
        std::env::var("TAKO_REDIS_URL")
            .ok()
            .or_else(|| Some("redis://127.0.0.1:6379".to_string()))
    }

    #[tokio::test]
    #[ignore]
    async fn live_round_trip() {
        let url = live_redis_url().unwrap();
        let key = format!("tako:sigstore:test:round_trip:{}", std::process::id());
        let store = RedisStateStore::connect(&url)
            .await
            .unwrap()
            .with_key(&key);
        StateStore::save(&store, 0).await.unwrap();
        assert_eq!(StateStore::load(&store).await.unwrap(), 0);
        StateStore::save(&store, 7).await.unwrap();
        assert_eq!(StateStore::load(&store).await.unwrap(), 7);
        let mut conn = store.manager.clone();
        let _: () = conn.del(&key).await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn live_first_boot_returns_zero() {
        let url = live_redis_url().unwrap();
        let key = format!("tako:sigstore:test:first_boot:{}", std::process::id());
        let store = RedisStateStore::connect(&url)
            .await
            .unwrap()
            .with_key(&key);
        let mut conn = store.manager.clone();
        let _: () = conn.del(&key).await.unwrap();
        assert_eq!(StateStore::load(&store).await.unwrap(), 0);
    }

    /// Phase 13.A core safety property: a stale replica writing a
    /// value below the current high-water mark MUST NOT clobber it.
    #[tokio::test]
    #[ignore]
    async fn live_save_is_monotonic() {
        let url = live_redis_url().unwrap();
        let key = format!("tako:sigstore:test:monotonic:{}", std::process::id());
        let store = RedisStateStore::connect(&url)
            .await
            .unwrap()
            .with_key(&key);
        StateStore::save(&store, 10).await.unwrap();
        StateStore::save(&store, 5).await.unwrap();
        assert_eq!(
            StateStore::load(&store).await.unwrap(),
            10,
            "stale write must not clobber a higher water-mark"
        );
        StateStore::save(&store, 12).await.unwrap();
        assert_eq!(StateStore::load(&store).await.unwrap(), 12);
        let mut conn = store.manager.clone();
        let _: () = conn.del(&key).await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn live_with_key_overrides_default() {
        let url = live_redis_url().unwrap();
        let key = format!("tako:sigstore:test:custom_key:{}", std::process::id());
        let store = RedisStateStore::connect(&url)
            .await
            .unwrap()
            .with_key(&key);
        assert_eq!(store.key(), key);
        StateStore::save(&store, 4).await.unwrap();
        let mut conn = store.manager.clone();
        let raw: Option<String> = conn.get(&key).await.unwrap();
        assert_eq!(raw.as_deref(), Some("4"));
        let _: () = conn.del(&key).await.unwrap();
    }
}
