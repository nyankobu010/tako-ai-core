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
    /// Phase 26 — when `true`, any [`TakoError::Transport`] from
    /// a child returns immediately instead of falling through to
    /// the next child. Default `false` preserves Phase 21
    /// fall-through-on-any-Err semantics byte-for-byte.
    short_circuit_on_transport_error: bool,
}

impl ChainedAuthResolver {
    /// Empty chain. `resolve` returns
    /// `TakoError::Invalid("chained auth: no resolvers configured")`
    /// until at least one child is added via [`Self::then`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a child resolver. Children are tried in append order;
    /// the first to return `Ok` short-circuits.
    ///
    /// Reads as "try `self`, **then** `child` if that fails" —
    /// matches the JS `Promise.then` and Rust `Future` `.then(...)`
    /// idiom for sequential composition. Avoids the Python `with`
    /// keyword clash that would prevent the Python facade from
    /// using the same method name.
    pub fn then(mut self, child: Arc<dyn AuthResolver>) -> Self {
        self.children.push(child);
        self
    }

    /// Phase 26 — opt in to fail-fast on transport errors. When
    /// enabled, a [`TakoError::Transport`] from any child halts
    /// the chain and propagates the error immediately, instead of
    /// falling through to the next child.
    ///
    /// Useful for the common "OIDC bearer OR static API key"
    /// pattern: when the OIDC issuer is unreachable, the operator
    /// wants the actionable `"transport error: ..."` to surface,
    /// not a misleading `"unknown bearer token"` from a fallback
    /// resolver. Other error variants
    /// ([`TakoError::Invalid`], [`TakoError::PolicyDenied`], etc.)
    /// continue to fall through — those represent auth decisions
    /// the next resolver might overturn.
    ///
    /// Idempotent. Default `false` preserves Phase 21
    /// fall-through-on-any-Err semantics byte-for-byte.
    pub fn with_short_circuit_on_transport_error(mut self) -> Self {
        self.short_circuit_on_transport_error = true;
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

    /// Phase 26 — accessor for the short-circuit flag, useful for
    /// assertions in test code.
    pub fn short_circuits_on_transport_error(&self) -> bool {
        self.short_circuit_on_transport_error
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
                Err(e) => {
                    // Phase 26 — short-circuit on transport errors
                    // when the operator opted in. Other error
                    // variants still fall through (auth-decision
                    // errors that the next resolver might
                    // overturn).
                    if self.short_circuit_on_transport_error && matches!(e, TakoError::Transport(_))
                    {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
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
            // a copy by inspecting the mutex contents. Phase 26
            // adds a `Transport` arm so short-circuit-semantics
            // tests can preserve the variant; other variants
            // collapse into `Invalid` (which the existing 21.A
            // tests already rely on).
            let guard = self.result.lock().expect("test mutex");
            match &*guard {
                Ok(p) => Ok(p.clone()),
                Err(TakoError::Transport(msg)) => Err(TakoError::Transport(msg.clone())),
                Err(TakoError::Invalid(msg)) => Err(TakoError::Invalid(msg.clone())),
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
        let chain = ChainedAuthResolver::new().then(Arc::new(inner));
        let p = chain.resolve("the-token").await.unwrap();
        assert_eq!(p.user_id, "alice");
    }

    #[tokio::test]
    async fn chained_first_match_short_circuits() {
        // First child returns Ok; the second must NOT be called.
        let first = CountingAuth::new(Ok(alice()));
        let second = CountingAuth::new(Ok(bob()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>);

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
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>);

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
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>);

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
        let inner = ChainedAuthResolver::new().then(Arc::new(leaf));
        let outer = ChainedAuthResolver::new().then(Arc::new(inner));
        let p = outer.resolve("the-token").await.unwrap();
        assert_eq!(p.user_id, "alice");
    }

    #[test]
    fn chained_len_and_is_empty_track_children() {
        let mut chain = ChainedAuthResolver::new();
        assert_eq!(chain.len(), 0);
        assert!(chain.is_empty());
        chain = chain.then(Arc::new(StaticTokens::new()));
        assert_eq!(chain.len(), 1);
        assert!(!chain.is_empty());
        chain = chain.then(Arc::new(StaticTokens::new()));
        assert_eq!(chain.len(), 2);
    }

    // -----------------------------------------------------------------
    // Phase 26 — fail-fast on transport errors. Default behaviour
    // (Phase 21 cadence) is unchanged: any `Err` falls through to
    // the next child. Operators opt in via
    // `with_short_circuit_on_transport_error` to halt the chain on
    // `TakoError::Transport`.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn short_circuit_default_falls_through_on_transport_error() {
        // Phase 21 regression pin — without
        // `with_short_circuit_on_transport_error`, transport
        // errors fall through to the next child exactly like
        // Invalid errors do.
        let first = CountingAuth::new(Err(TakoError::Transport("oidc unreachable".into())));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>);

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(first.call_count(), 1);
        assert_eq!(second.call_count(), 1);
    }

    #[tokio::test]
    async fn short_circuit_enabled_returns_immediately_on_transport_error() {
        // Phase 26 — first child returns `Transport`; the second
        // child must NOT be called; the transport error
        // propagates verbatim.
        let first = CountingAuth::new(Err(TakoError::Transport("oidc unreachable".into())));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_transport_error();

        let err = chain.resolve("any").await.unwrap_err();
        assert!(matches!(err, TakoError::Transport(_)), "got: {err:?}");
        let msg = format!("{err:?}");
        assert!(msg.contains("oidc unreachable"), "got: {msg}");
        assert_eq!(first.call_count(), 1);
        assert_eq!(
            second.call_count(),
            0,
            "second child must not be called when the first short-circuits on Transport",
        );
    }

    #[tokio::test]
    async fn short_circuit_enabled_falls_through_on_invalid_error() {
        // Phase 26 — `TakoError::Invalid` is an auth decision
        // (token bad, signature mismatch); it falls through to
        // the next resolver even when short-circuit is enabled.
        // Only `Transport` short-circuits.
        let first = CountingAuth::new(Err(TakoError::Invalid("bad token".into())));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_transport_error();

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(first.call_count(), 1);
        assert_eq!(second.call_count(), 1);
    }

    #[tokio::test]
    async fn short_circuit_enabled_first_ok_still_short_circuits_happy_path() {
        // Regression pin — the happy path is unchanged. First
        // child returns `Ok`; the second is not called. Short-
        // circuit-on-transport-error doesn't affect normal
        // first-Ok behaviour.
        let first = CountingAuth::new(Ok(alice()));
        let second = CountingAuth::new(Ok(bob()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_transport_error();

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(first.call_count(), 1);
        assert_eq!(second.call_count(), 0);
    }

    #[test]
    fn short_circuits_on_transport_error_accessor_reflects_state() {
        let chain = ChainedAuthResolver::new();
        assert!(!chain.short_circuits_on_transport_error());
        let chain = chain.with_short_circuit_on_transport_error();
        assert!(chain.short_circuits_on_transport_error());
        // Idempotent — calling twice doesn't break.
        let chain = chain.with_short_circuit_on_transport_error();
        assert!(chain.short_circuits_on_transport_error());
    }
}
