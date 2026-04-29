//! Redis-backed [`BudgetBackend`].
//!
//! Per-tenant cumulative spend is keyed by `<prefix>:{tenant_id}:{YYYY-MM-DD}`
//! (UTC), with `usd` (float) and `tokens` (int) fields stored in a Redis
//! hash. Day rollover is automatic: tomorrow's writes land in a fresh
//! key; today's key is auto-evicted by Redis once its TTL elapses
//! (default 48h, so concurrent reconciles after midnight still find a
//! live key for the previous day).
//!
//! `record()` is atomic via a small `EVAL` script: `HINCRBYFLOAT` on
//! `usd`, `HINCRBY` on `tokens`, and `EXPIRE` on the key — three calls
//! collapsed into one round-trip with no torn writes.
//!
//! Gated behind the `redis` Cargo feature so the `redis` crate (and its
//! TLS / async-runtime infrastructure) only land in the dep tree when
//! explicitly enabled.

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use redis::{AsyncCommands, Script, aio::ConnectionManager};
use tako_core::TakoError;

use crate::budget::{BudgetBackend, TenantUsage};

const DEFAULT_KEY_PREFIX: &str = "tako:budget";
const DEFAULT_TTL_SECS: u64 = 48 * 60 * 60;

/// Atomic record script. KEYS[1]=hash key, ARGV[1]=usd float,
/// ARGV[2]=tokens int, ARGV[3]=ttl seconds.
const RECORD_LUA: &str = r#"
redis.call('HINCRBYFLOAT', KEYS[1], 'usd', ARGV[1])
redis.call('HINCRBY', KEYS[1], 'tokens', ARGV[2])
redis.call('EXPIRE', KEYS[1], ARGV[3])
return 1
"#;

/// Redis-backed [`BudgetBackend`] suitable for multi-process deployments.
#[derive(Clone)]
pub struct RedisBudgetBackend {
    manager: ConnectionManager,
    key_prefix: String,
    ttl_secs: u64,
    record_script: Script,
}

impl std::fmt::Debug for RedisBudgetBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisBudgetBackend")
            .field("key_prefix", &self.key_prefix)
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

impl RedisBudgetBackend {
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
            key_prefix: DEFAULT_KEY_PREFIX.into(),
            ttl_secs: DEFAULT_TTL_SECS,
            record_script: Script::new(RECORD_LUA),
        })
    }

    /// Override the key prefix (default `"tako:budget"`).
    pub fn with_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = prefix.into();
        self
    }

    /// Override the per-key TTL (default 48 hours). Must be at least 24h
    /// to survive day rollovers in face of concurrent reconciles.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl_secs = ttl.as_secs();
        self
    }

    fn day_key(&self, tenant_id: &str) -> String {
        Self::format_day_key(&self.key_prefix, tenant_id, Utc::now().date_naive())
    }

    fn format_day_key(prefix: &str, tenant_id: &str, day: chrono::NaiveDate) -> String {
        format!("{prefix}:{tenant_id}:{}", day.format("%Y-%m-%d"))
    }
}

#[async_trait]
impl BudgetBackend for RedisBudgetBackend {
    async fn current_usage(&self, tenant_id: &str) -> Result<TenantUsage, TakoError> {
        let key = self.day_key(tenant_id);
        let mut conn = self.manager.clone();
        // HGETALL on a missing key returns an empty map, so zero-usage
        // is the natural default with no extra branching.
        let raw: std::collections::HashMap<String, String> = conn
            .hgetall(&key)
            .await
            .map_err(|e| TakoError::Transport(format!("redis hgetall: {e}")))?;
        let usd_today: f64 = raw.get("usd").and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let tokens_today: u64 = raw.get("tokens").and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok(TenantUsage {
            usd_today,
            tokens_today,
        })
    }

    async fn record(&self, tenant_id: &str, usd: f64, tokens: u64) -> Result<(), TakoError> {
        let key = self.day_key(tenant_id);
        let mut conn = self.manager.clone();
        self.record_script
            .key(&key)
            .arg(usd)
            .arg(tokens)
            .arg(self.ttl_secs)
            .invoke_async::<i64>(&mut conn)
            .await
            .map_err(|e| TakoError::Transport(format!("redis eval: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn day_key_format_is_stable() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 4, 29).unwrap();
        let key = RedisBudgetBackend::format_day_key("tako:budget", "acme", day);
        assert_eq!(key, "tako:budget:acme:2026-04-29");
    }

    #[test]
    fn day_key_supports_custom_prefix_and_unicode_tenant() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        let key = RedisBudgetBackend::format_day_key("budgets:v2", "tenant-蛸", day);
        assert_eq!(key, "budgets:v2:tenant-蛸:2026-12-31");
    }
}
