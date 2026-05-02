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

/// Phase 26 / 27 — short-circuit policy for a
/// [`ChainedAuthResolver`]. Selects which `TakoError` variants
/// halt the chain instead of falling through to the next child.
///
/// Default [`Self::None`] preserves Phase 21
/// fall-through-on-any-Err semantics byte-for-byte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum ShortCircuitPolicy {
    /// Phase 21 default — every `Err` falls through to the next
    /// child.
    #[default]
    None,
    /// Phase 26 — short-circuit only on
    /// [`TakoError::Transport`].
    TransportOnly,
    /// Phase 27 — short-circuit on the four "definitely
    /// infrastructure / operator-set guard" variants:
    /// `Transport`, `RateLimited`, `CircuitOpen`,
    /// `BudgetExhausted`. Auth-decision errors (`Invalid`,
    /// `PolicyDenied`) and vendor errors (`Provider`) still fall
    /// through.
    AllInfrastructure,
}

/// Phase 36 — per-child override for the chain-wide
/// short-circuit policy set by Phase 26 / 27 builders.
///
/// Default [`Self::Inherit`] preserves Phase 21 / 26 / 27
/// chain-wide semantics byte-for-byte: the per-child override
/// is inert unless explicitly set.
///
/// Real deployments often mix critical primary backends (OIDC
/// issuer; transport / rate-limit / circuit failures should
/// halt the chain) with graceful-tail fallbacks (in-process
/// static API keys; never short-circuit). Without per-child
/// override, the chain-wide flag forces a single sensitivity
/// level on all children.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ChildShortCircuitPolicy {
    /// Inherit the chain-wide policy. Default — equivalent to
    /// the Phase 21 `then(child)` builder.
    #[default]
    Inherit,
    /// Override: every `Err` from this child falls through to
    /// the next, regardless of chain-wide policy. Useful for
    /// graceful-tail fallbacks (in-process static-token
    /// last-resort that must keep serving even when the
    /// chain-wide flag is set).
    AlwaysFallThrough,
    /// Override: short-circuit only on
    /// [`TakoError::Transport`]. Narrower than chain-wide
    /// `AllInfrastructure`.
    TransportOnly,
    /// Override: short-circuit on `Transport` /
    /// `RateLimited` / `CircuitOpen` / `BudgetExhausted`.
    /// Broader than chain-wide `TransportOnly`.
    AllInfrastructure,
}

/// Internal child entry. Phase 36 widens the `Vec<Arc<dyn
/// AuthResolver>>` to carry per-child policy.
#[derive(Clone, Debug)]
struct ChildEntry {
    resolver: Arc<dyn AuthResolver>,
    policy: ChildShortCircuitPolicy,
}

/// Phase 21.A — try children in order until one returns `Ok`.
#[derive(Clone, Debug, Default)]
pub struct ChainedAuthResolver {
    children: Vec<ChildEntry>,
    /// Phase 26 / 27 — selects which error variants halt the
    /// chain immediately instead of falling through to the next
    /// child. Default `ShortCircuitPolicy::None` preserves Phase
    /// 21 fall-through-on-any-Err semantics byte-for-byte.
    /// Phase 36 lets individual children override this via
    /// [`ChildShortCircuitPolicy`].
    short_circuit_policy: ShortCircuitPolicy,
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
    ///
    /// Phase 36: equivalent to
    /// [`Self::then_with_short_circuit`] with
    /// [`ChildShortCircuitPolicy::Inherit`].
    pub fn then(mut self, child: Arc<dyn AuthResolver>) -> Self {
        self.children.push(ChildEntry {
            resolver: child,
            policy: ChildShortCircuitPolicy::Inherit,
        });
        self
    }

    /// Phase 36 — append a child WITH a per-child
    /// short-circuit-policy override.
    ///
    /// The chain-wide policy (set by [`Self::with_short_circuit_on_transport_error`]
    /// / [`Self::with_short_circuit_on_infrastructure_errors`])
    /// still applies to every child whose own override is
    /// [`ChildShortCircuitPolicy::Inherit`] — so the existing
    /// [`Self::then`] keeps Phase 21 / 26 / 27 cadence
    /// byte-for-byte.
    ///
    /// Override priority: when a child's
    /// [`ChildShortCircuitPolicy`] is anything other than
    /// `Inherit`, that policy alone determines whether the
    /// child's error halts the chain — the chain-wide flag is
    /// ignored for this child.
    pub fn then_with_short_circuit(
        mut self,
        child: Arc<dyn AuthResolver>,
        policy: ChildShortCircuitPolicy,
    ) -> Self {
        self.children.push(ChildEntry {
            resolver: child,
            policy,
        });
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
    /// Idempotent. Default behaviour preserves Phase 21
    /// fall-through-on-any-Err semantics byte-for-byte.
    /// Last-write-wins between this method and Phase 27's
    /// [`Self::with_short_circuit_on_infrastructure_errors`] —
    /// the policy is overwritten, not merged.
    pub fn with_short_circuit_on_transport_error(mut self) -> Self {
        self.short_circuit_policy = ShortCircuitPolicy::TransportOnly;
        self
    }

    /// Phase 27 — broader fail-fast: short-circuit on
    /// infrastructure / operator-set-guard errors that masking
    /// via fall-through would hide:
    /// - [`TakoError::Transport`] (network failure)
    /// - [`TakoError::RateLimited`] (operator-side limit)
    /// - [`TakoError::CircuitOpen`] (failsafe circuit)
    /// - [`TakoError::BudgetExhausted`] (operator-set spend cap)
    ///
    /// Auth-decision errors ([`TakoError::Invalid`],
    /// [`TakoError::PolicyDenied`]) and vendor errors
    /// ([`TakoError::Provider`]) continue to fall through — those
    /// could be auth-related and the next resolver might overturn.
    /// `Provider` short-circuit warrants finer discrimination on
    /// the embedded error and is deferred.
    ///
    /// Useful when `RateLimited` / `CircuitOpen` /
    /// `BudgetExhausted` from one resolver shouldn't be masked by
    /// fall-through to another (each represents an infrastructure
    /// failure or an operator-set guard that falling through
    /// would circumvent).
    ///
    /// Idempotent. Last-write-wins between this method and Phase
    /// 26's [`Self::with_short_circuit_on_transport_error`] — the
    /// policy is overwritten, not merged.
    pub fn with_short_circuit_on_infrastructure_errors(mut self) -> Self {
        self.short_circuit_policy = ShortCircuitPolicy::AllInfrastructure;
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

    /// Phase 26 — accessor: returns `true` for both
    /// [`Self::with_short_circuit_on_transport_error`] and Phase
    /// 27's [`Self::with_short_circuit_on_infrastructure_errors`]
    /// (both short-circuit on `Transport`).
    pub fn short_circuits_on_transport_error(&self) -> bool {
        !matches!(self.short_circuit_policy, ShortCircuitPolicy::None)
    }

    /// Phase 27 — accessor for the broader policy. Returns
    /// `true` only when
    /// [`Self::with_short_circuit_on_infrastructure_errors`] was
    /// the most recent policy setter.
    pub fn short_circuits_on_infrastructure_errors(&self) -> bool {
        matches!(
            self.short_circuit_policy,
            ShortCircuitPolicy::AllInfrastructure
        )
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
        for entry in &self.children {
            match entry.resolver.resolve(token).await {
                Ok(p) => return Ok(p),
                Err(e) => {
                    // Phase 36 — per-child policy overrides the
                    // chain-wide policy when it's anything other
                    // than `Inherit`. Phase 26 / 27 chain-wide
                    // semantics still drive `Inherit` children
                    // byte-for-byte.
                    let should_short_circuit = match entry.policy {
                        ChildShortCircuitPolicy::Inherit => {
                            chain_wide_short_circuit(&self.short_circuit_policy, &e)
                        }
                        ChildShortCircuitPolicy::AlwaysFallThrough => false,
                        ChildShortCircuitPolicy::TransportOnly => {
                            matches!(e, TakoError::Transport(_))
                        }
                        ChildShortCircuitPolicy::AllInfrastructure => is_infrastructure_error(&e),
                    };
                    if should_short_circuit {
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

/// Phase 26 / 27 chain-wide short-circuit predicate. Auth-decision
/// errors (`Invalid`, `PolicyDenied`) and vendor errors
/// (`Provider`) always fall through. Phase 36 routes `Inherit`
/// children through this helper.
fn chain_wide_short_circuit(policy: &ShortCircuitPolicy, e: &TakoError) -> bool {
    match policy {
        ShortCircuitPolicy::None => false,
        ShortCircuitPolicy::TransportOnly => matches!(e, TakoError::Transport(_)),
        ShortCircuitPolicy::AllInfrastructure => is_infrastructure_error(e),
    }
}

/// Phase 27's "definitely infrastructure / operator-set guard"
/// set: `Transport` (network failure), `RateLimited` (operator-side
/// limit), `CircuitOpen` (failsafe circuit), `BudgetExhausted`
/// (operator-set spend cap). Pulled into a helper in Phase 36 so
/// chain-wide and per-child code paths agree on the set.
fn is_infrastructure_error(e: &TakoError) -> bool {
    matches!(
        e,
        TakoError::Transport(_)
            | TakoError::RateLimited(_)
            | TakoError::CircuitOpen
            | TakoError::BudgetExhausted(_)
    )
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
            // a copy by inspecting the mutex contents. Phases 26 +
            // 27 preserve `Transport` / `RateLimited` /
            // `CircuitOpen` / `BudgetExhausted` so
            // short-circuit-semantics tests can preserve the
            // variant; other variants collapse into `Invalid`
            // (which the Phase 21 tests rely on).
            let guard = self.result.lock().expect("test mutex");
            match &*guard {
                Ok(p) => Ok(p.clone()),
                Err(TakoError::Transport(msg)) => Err(TakoError::Transport(msg.clone())),
                Err(TakoError::RateLimited(d)) => Err(TakoError::RateLimited(*d)),
                Err(TakoError::CircuitOpen) => Err(TakoError::CircuitOpen),
                Err(TakoError::BudgetExhausted(msg)) => {
                    Err(TakoError::BudgetExhausted(msg.clone()))
                }
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

    // -----------------------------------------------------------------
    // Phase 27 — broader infrastructure-error short-circuit. Adds
    // `RateLimited` / `CircuitOpen` / `BudgetExhausted` to the set
    // of variants that halt the chain when the operator opts in
    // via `with_short_circuit_on_infrastructure_errors`.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn infrastructure_short_circuit_default_falls_through_on_rate_limited() {
        // Phase 21 / 26 regression — without
        // `with_short_circuit_on_infrastructure_errors`,
        // `RateLimited` falls through to the next child.
        let first = CountingAuth::new(Err(TakoError::RateLimited(std::time::Duration::from_secs(
            60,
        ))));
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
    async fn infrastructure_short_circuit_returns_immediately_on_rate_limited() {
        let first = CountingAuth::new(Err(TakoError::RateLimited(std::time::Duration::from_secs(
            60,
        ))));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_infrastructure_errors();

        let err = chain.resolve("any").await.unwrap_err();
        assert!(matches!(err, TakoError::RateLimited(_)), "got: {err:?}");
        assert_eq!(first.call_count(), 1);
        assert_eq!(
            second.call_count(),
            0,
            "second child must not be called when the first short-circuits on RateLimited",
        );
    }

    #[tokio::test]
    async fn infrastructure_short_circuit_returns_immediately_on_circuit_open() {
        let first = CountingAuth::new(Err(TakoError::CircuitOpen));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_infrastructure_errors();

        let err = chain.resolve("any").await.unwrap_err();
        assert!(matches!(err, TakoError::CircuitOpen), "got: {err:?}");
        assert_eq!(second.call_count(), 0);
    }

    #[tokio::test]
    async fn infrastructure_short_circuit_returns_immediately_on_budget_exhausted() {
        let first = CountingAuth::new(Err(TakoError::BudgetExhausted("daily cap hit".into())));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_infrastructure_errors();

        let err = chain.resolve("any").await.unwrap_err();
        assert!(matches!(err, TakoError::BudgetExhausted(_)), "got: {err:?}");
        let msg = format!("{err:?}");
        assert!(msg.contains("daily cap hit"), "got: {msg}");
        assert_eq!(second.call_count(), 0);
    }

    #[tokio::test]
    async fn infrastructure_short_circuit_falls_through_on_invalid_error() {
        // Auth-decision errors (`Invalid`, `PolicyDenied`,
        // `Provider`) must still fall through, even with the
        // broader policy enabled.
        let first = CountingAuth::new(Err(TakoError::Invalid("bad token".into())));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_infrastructure_errors();

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(first.call_count(), 1);
        assert_eq!(second.call_count(), 1);
    }

    #[tokio::test]
    async fn transport_only_falls_through_on_rate_limited_when_transport_only_set() {
        // Regression pin: the Phase-26 narrower flag does NOT
        // short-circuit on `RateLimited` even after the policy
        // enum refactor. Falls through like any non-Transport
        // error.
        let first = CountingAuth::new(Err(TakoError::RateLimited(std::time::Duration::from_secs(
            60,
        ))));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(first.clone() as Arc<dyn AuthResolver>)
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_transport_error();

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(second.call_count(), 1);
    }

    #[test]
    fn short_circuits_on_infrastructure_errors_accessor_reflects_state() {
        let chain = ChainedAuthResolver::new();
        assert!(!chain.short_circuits_on_transport_error());
        assert!(!chain.short_circuits_on_infrastructure_errors());

        // Phase 26 narrower flag: transport accessor true, infra
        // accessor false.
        let narrow = chain.clone().with_short_circuit_on_transport_error();
        assert!(narrow.short_circuits_on_transport_error());
        assert!(!narrow.short_circuits_on_infrastructure_errors());

        // Phase 27 broader flag: both accessors true.
        let broad = chain.with_short_circuit_on_infrastructure_errors();
        assert!(broad.short_circuits_on_transport_error());
        assert!(broad.short_circuits_on_infrastructure_errors());
    }

    #[test]
    fn short_circuit_policy_is_last_write_wins() {
        // Calling the broader builder after the narrower one
        // overwrites the policy (and vice versa).
        let chain = ChainedAuthResolver::new()
            .with_short_circuit_on_transport_error()
            .with_short_circuit_on_infrastructure_errors();
        assert!(chain.short_circuits_on_infrastructure_errors());

        let chain = ChainedAuthResolver::new()
            .with_short_circuit_on_infrastructure_errors()
            .with_short_circuit_on_transport_error();
        assert!(!chain.short_circuits_on_infrastructure_errors());
        assert!(chain.short_circuits_on_transport_error());
    }

    // -----------------------------------------------------------------
    // Phase 36 — per-child short-circuit policy override.
    //
    // Adds `then_with_short_circuit(child, ChildShortCircuitPolicy)`
    // for marking individual children with a different sensitivity
    // than the chain-wide policy. Bare `then(...)` keeps the Phase
    // 21 cadence (defaults to `ChildShortCircuitPolicy::Inherit`).
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn per_child_always_fall_through_overrides_chain_wide_infra() {
        // Chain-wide infra short-circuit, but the first child is
        // marked AlwaysFallThrough — `RateLimited` still falls
        // through to the second.
        let first = CountingAuth::new(Err(TakoError::RateLimited(std::time::Duration::from_secs(
            60,
        ))));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then_with_short_circuit(
                first.clone() as Arc<dyn AuthResolver>,
                ChildShortCircuitPolicy::AlwaysFallThrough,
            )
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_infrastructure_errors();

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(first.call_count(), 1);
        assert_eq!(
            second.call_count(),
            1,
            "AlwaysFallThrough must override chain-wide infra short-circuit"
        );
    }

    #[tokio::test]
    async fn per_child_transport_only_overrides_chain_wide_infra() {
        // Chain-wide infra short-circuit, but the first child is
        // marked TransportOnly — `RateLimited` falls through (the
        // narrower per-child policy doesn't include it), while
        // `Transport` would halt.
        let first = CountingAuth::new(Err(TakoError::RateLimited(std::time::Duration::from_secs(
            30,
        ))));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then_with_short_circuit(
                first.clone() as Arc<dyn AuthResolver>,
                ChildShortCircuitPolicy::TransportOnly,
            )
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_infrastructure_errors();

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(first.call_count(), 1);
        assert_eq!(second.call_count(), 1);
    }

    #[tokio::test]
    async fn per_child_transport_only_still_halts_on_transport() {
        let first = CountingAuth::new(Err(TakoError::Transport("oidc unreachable".into())));
        let second = CountingAuth::new(Ok(alice()));
        // Chain-wide is None (Phase 21 default), but per-child
        // TransportOnly halts.
        let chain = ChainedAuthResolver::new()
            .then_with_short_circuit(
                first.clone() as Arc<dyn AuthResolver>,
                ChildShortCircuitPolicy::TransportOnly,
            )
            .then(second.clone() as Arc<dyn AuthResolver>);

        let err = chain.resolve("any").await.unwrap_err();
        assert!(matches!(err, TakoError::Transport(_)), "got: {err:?}");
        assert_eq!(second.call_count(), 0);
    }

    #[tokio::test]
    async fn per_child_all_infrastructure_overrides_chain_wide_transport_only() {
        // Chain-wide TransportOnly, but the first child is
        // marked AllInfrastructure — `RateLimited` halts the
        // chain because the per-child policy is broader.
        let first = CountingAuth::new(Err(TakoError::RateLimited(std::time::Duration::from_secs(
            30,
        ))));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then_with_short_circuit(
                first.clone() as Arc<dyn AuthResolver>,
                ChildShortCircuitPolicy::AllInfrastructure,
            )
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_transport_error();

        let err = chain.resolve("any").await.unwrap_err();
        assert!(matches!(err, TakoError::RateLimited(_)), "got: {err:?}");
        assert_eq!(
            second.call_count(),
            0,
            "per-child AllInfrastructure must halt on RateLimited even when chain-wide is TransportOnly"
        );
    }

    #[tokio::test]
    async fn per_child_inherit_default_preserves_chain_wide() {
        // `then_with_short_circuit(child, Inherit)` is identical
        // to `then(child)` — chain-wide TransportOnly applies.
        let first = CountingAuth::new(Err(TakoError::Transport("oidc unreachable".into())));
        let second = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then_with_short_circuit(
                first.clone() as Arc<dyn AuthResolver>,
                ChildShortCircuitPolicy::Inherit,
            )
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_transport_error();

        let err = chain.resolve("any").await.unwrap_err();
        assert!(matches!(err, TakoError::Transport(_)), "got: {err:?}");
        assert_eq!(second.call_count(), 0);
    }

    #[tokio::test]
    async fn per_child_policy_does_not_affect_happy_path() {
        // Even with a non-default per-child policy, an `Ok` from
        // the first child still short-circuits the chain.
        let first = CountingAuth::new(Ok(alice()));
        let second = CountingAuth::new(Ok(bob()));
        let chain = ChainedAuthResolver::new()
            .then_with_short_circuit(
                first.clone() as Arc<dyn AuthResolver>,
                ChildShortCircuitPolicy::AlwaysFallThrough,
            )
            .then(second.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_infrastructure_errors();

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(first.call_count(), 1);
        assert_eq!(second.call_count(), 0);
    }

    #[tokio::test]
    async fn then_and_then_with_short_circuit_can_mix() {
        // Operator builds a 3-child chain mixing both builders:
        // OIDC (Inherit) → JWT (AlwaysFallThrough) → static
        // (Inherit). Chain-wide is infra short-circuit. JWT
        // returns `RateLimited` but its AlwaysFallThrough
        // override forces fall-through; static then succeeds.
        let oidc = CountingAuth::new(Err(TakoError::Invalid("bad token".into())));
        let jwt = CountingAuth::new(Err(TakoError::RateLimited(std::time::Duration::from_secs(
            10,
        ))));
        let static_tail = CountingAuth::new(Ok(alice()));
        let chain = ChainedAuthResolver::new()
            .then(oidc.clone() as Arc<dyn AuthResolver>)
            .then_with_short_circuit(
                jwt.clone() as Arc<dyn AuthResolver>,
                ChildShortCircuitPolicy::AlwaysFallThrough,
            )
            .then(static_tail.clone() as Arc<dyn AuthResolver>)
            .with_short_circuit_on_infrastructure_errors();

        let p = chain.resolve("any").await.unwrap();
        assert_eq!(p.user_id, "alice");
        assert_eq!(oidc.call_count(), 1);
        assert_eq!(jwt.call_count(), 1);
        assert_eq!(static_tail.call_count(), 1);
    }
}
