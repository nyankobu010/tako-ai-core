//! `ChainedAuthResolver` — Phase 21.A composite [`AuthResolver`] that
//! tries N children in order and returns the first `Ok`.
//!
//! Common operator pattern this addresses: "accept either an OIDC
//! bearer token OR a static API key" — typical when migrating from a
//! handcurated API-key list to an OIDC issuer. Today operators have
//! to pick a single `auth=` resolver on
//! [`crate::serve_openai`]; this resolver lets them compose any
//! combination of [`StaticTokens`](super::StaticTokens),
//! [`JwtAuthResolver`](super::JwtAuthResolver),
//! [`OidcAuthResolver`](super::OidcAuthResolver), and
//! [`VaultAuthResolver`](super::VaultAuthResolver).
//!
//! Semantics:
//! - **Empty chain:** `resolve` returns
//!   `TakoError::Invalid("chained auth: no resolvers configured")`.
//! - **Children tried in append order.** The first to return `Ok`
//!   short-circuits.
//! - **Any `Err` falls through** to the next child. Transient OIDC
//!   transport failures don't strand a static-API-key client.
//! - **All-`Err`:** the last child's error is returned.
//!
//! No feature gate — `ChainedAuthResolver` is always available
//! because the [`AuthResolver`] trait is always available; children
//! themselves bring whatever `auth-*` cargo features they were built
//! under.

use std::sync::Arc;

use async_trait::async_trait;
use tako_core::{Principal, TakoError};

use super::AuthResolver;

/// Phase 21.A — try children in order until one returns `Ok`.
#[derive(Clone, Debug, Default)]
pub struct ChainedAuthResolver {
    children: Vec<Arc<dyn AuthResolver>>,
}

impl ChainedAuthResolver {
    /// Empty chain. `resolve` returns
    /// `TakoError::Invalid("chained auth: no resolvers configured")`
    /// until at least one child is added via [`Self::with`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a child resolver. Children are tried in append order;
    /// the first to return `Ok` short-circuits.
    pub fn with(mut self, child: Arc<dyn AuthResolver>) -> Self {
        self.children.push(child);
        self
    }

    /// Number of children in the chain.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// `true` when no children have been added.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

#[async_trait]
impl AuthResolver for ChainedAuthResolver {
    async fn resolve(&self, token: &str) -> Result<Principal, TakoError> {
        if self.children.is_empty() {
            return Err(TakoError::Invalid(
                "chained auth: no resolvers configured".into(),
            ));
        }
        let mut last_err: Option<TakoError> = None;
        for child in &self.children {
            match child.resolve(token).await {
                Ok(p) => return Ok(p),
                Err(e) => last_err = Some(e),
            }
        }
        // Safe: children non-empty above, so the loop ran at least
        // once and at least one Err was produced (otherwise we'd
        // have returned Ok early).
        Err(last_err.unwrap_or_else(|| {
            TakoError::Invalid("chained auth: unreachable empty-error path".into())
        }))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::auth::StaticTokens;

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    /// Counting mock resolver — returns the configured `result`
    /// on every call and increments a per-instance call counter.
    /// Used by `chained_first_match_short_circuits` to assert the
    /// second child is NOT called when the first returns `Ok`.
    #[derive(Debug)]
    struct CountingAuth {
        calls: AtomicUsize,
        result: std::sync::Mutex<Result<Principal, TakoError>>,
    }

    impl CountingAuth {
        fn new(result: Result<Principal, TakoError>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                result: std::sync::Mutex::new(result),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl AuthResolver for CountingAuth {
        async fn resolve(&self, _token: &str) -> Result<Principal, TakoError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            // `Result` isn't `Clone` for arbitrary `TakoError`, but
            // we only need to return a fresh value per call. Build
            // a copy by inspecting the mutex contents.
            let guard = self.result.lock().expect("test mutex");
            match &*guard {
                Ok(p) => Ok(p.clone()),
                Err(e) => Err(TakoError::Invalid(format!("{e:?}"))),
            }
        }
    }

    fn alice() -> Principal {
        Principal::new("acme", "alice")
    }

    fn bob() -> Principal {
        Principal::new("acme", "bob")
    }

    #[test]
    fn chained_is_send_sync_clone_debug() {
        assert_send_sync::<ChainedAuthResolver>();
        let c = ChainedAuthResolver::new();
        let _cloned = c.clone();
        let _dbg = format!("{c:?}");
    }

    #[tokio::test]
    async fn chained_empty_returns_invalid() {
        let chain = ChainedAuthResolver::new();
        let err = chain.resolve("anything").await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("no resolvers configured"), "got: {msg}");
    }

    #[tokio::test]
    async fn chained_single_pass_through() {
        let inner = StaticTokens::new().with("the-token", alice());
        let chain = ChainedAuthResolver::new().with(Arc::new(inner));
        let p = chain.resolve("the-token").await.unwrap();
        assert_eq!(p.user_id, "alice");
    }

    #[tokio::test]
    async fn chained_first_match_short_circuits() {
        // First child returns Ok; the second must NOT be called.
        let first = CountingAuth::new(Ok(alice()));
        let second = CountingAuth::new(Ok(bob()));
        let chain = ChainedAuthResolver::new()
            .with(first.clone() as Arc<dyn AuthResolver>)
            .with(second.clone() as Arc<dyn AuthResolver>);

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(first.call_count(), 1);
        assert_eq!(
            second.call_count(),
            0,
            "second child must not be called when first short-circuits",
        );
    }

    #[tokio::test]
    async fn chained_falls_through_to_second_when_first_errors() {
        let first = CountingAuth::new(Err(TakoError::Invalid("first failed".into())));
        let second = CountingAuth::new(Ok(bob()));
        let chain = ChainedAuthResolver::new()
            .with(first.clone() as Arc<dyn AuthResolver>)
            .with(second.clone() as Arc<dyn AuthResolver>);

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "bob");
        assert_eq!(first.call_count(), 1);
        assert_eq!(second.call_count(), 1);
    }

    #[tokio::test]
    async fn chained_returns_last_error_when_all_fail() {
        let first = CountingAuth::new(Err(TakoError::Invalid("first failed".into())));
        let second = CountingAuth::new(Err(TakoError::Invalid("second failed".into())));
        let chain = ChainedAuthResolver::new()
            .with(first.clone() as Arc<dyn AuthResolver>)
            .with(second.clone() as Arc<dyn AuthResolver>);

        let err = chain.resolve("any").await.unwrap_err();
        let msg = format!("{err:?}");
        // The last child's error must propagate (not the first's).
        assert!(msg.contains("second failed"), "got: {msg}");
        assert!(!msg.contains("first failed"), "got: {msg}");
    }

    #[tokio::test]
    async fn chained_can_nest() {
        // Recursive composition: a chain whose child is itself a
        // chain. Useful when building auth policies in layers.
        let leaf = StaticTokens::new().with("the-token", alice());
        let inner = ChainedAuthResolver::new().with(Arc::new(leaf));
        let outer = ChainedAuthResolver::new().with(Arc::new(inner));
        let p = outer.resolve("the-token").await.unwrap();
        assert_eq!(p.user_id, "alice");
    }

    #[test]
    fn chained_len_and_is_empty_track_children() {
        let mut chain = ChainedAuthResolver::new();
        assert_eq!(chain.len(), 0);
        assert!(chain.is_empty());
        chain = chain.with(Arc::new(StaticTokens::new()));
        assert_eq!(chain.len(), 1);
        assert!(!chain.is_empty());
        chain = chain.with(Arc::new(StaticTokens::new()));
        assert_eq!(chain.len(), 2);
    }
}
