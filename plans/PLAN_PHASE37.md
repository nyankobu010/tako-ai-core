# PLAN — Phase 37 (Trait-based `MtlsIdentityProvider` for proactive expiry-driven refresh)

> **Status: in progress.** Targets v0.38.0. Stacks on top of
> Phase 36 (v0.37.0); rebases onto main once
> [tako-ai-core#33](https://github.com/nyankobu010/tako-ai-core/pull/33)
> and [tako-ai-core#34](https://github.com/nyankobu010/tako-ai-core/pull/34)
> merge. Carry-forward strategy (1-of-2-remaining) from
> [Phase 33](PLAN_PHASE33.md).

## Context

Phase 33 (v0.34.0) shipped explicit-reload via
`OidcAuthResolver::reload_mtls_identity(cert_pem, key_pem)`.
Phase 35 (v0.36.0) shipped strategy 3 — filesystem-watcher
auto-reload via `watch_mtls_files(cert_path, key_path)` +
`notify`. Two strategies remained:

1. **Trait-based identity provider** — operator implements an
   async trait that yields fresh cert+key bytes; tako parses
   the cert's expiry and proactively re-fetches at e.g. 80%
   of validity. **This phase.**
2. **Automatic refresh-on-handshake-failure** — catch TLS
   handshake errors at request time, trigger a forced refresh,
   retry. Sits on top of (1) or the Phase 35 watcher. Still
   deferred — the Phase 37 trait shape is the cleanest base
   for it, so this is now Phase 38+.

The Phase 35 filesystem watcher works for the cert-manager /
kubernetes-secret-mount / Vault PKI **disk-based** rotation
patterns. It doesn't fit deployments where the cert+key live
somewhere other than the filesystem:

- **HSM-backed keys** — the private key never leaves the HSM;
  cert is rotated via a vendor SDK call rather than a file
  rewrite.
- **In-memory secret stores** — operator's app fetches
  cert+key from a vault/broker on demand; no file ever exists.
- **On-demand identity brokers** — SPIFFE Workload API,
  AWS IAM Roles Anywhere, or similar where the operator's app
  re-issues short-lived certs by calling a service.

For these deployments, operators want to plug in custom logic.
Phase 37 ships the trait surface: an `MtlsIdentityProvider`
async trait yielding `MtlsIdentity { cert_pem, key_pem }`, plus
a builder `OidcAuthResolver::watch_mtls_provider(provider)`
that spawns a background tokio task. The task parses the
returned cert's `NotAfter`, sleeps until 80% of the validity
window, then re-fetches.

## Why now

Both Phase 33 strategies that survive past Phase 35 are useful;
trait-based is the next natural carry-forward. Picking it now
unblocks Phase 38's "automatic refresh-on-handshake-failure"
which can sit on top of either the Phase 35 watcher or the
Phase 37 trait. After Phase 37 the Phase 33 carry-forward list
shrinks to just the handshake-failure layer.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 37.A | `MtlsIdentityProvider` async trait + `MtlsIdentity` PEM-pair value type | [`crates/tako-compat/src/auth/oidc_mtls_provider.rs`](../crates/tako-compat/src/auth/oidc_mtls_provider.rs) (new) |
| 37.B | `x509-parser` dep + cert NotAfter parsing helper, behind `mtls-identity-provider` cargo feature | [`crates/tako-compat/Cargo.toml`](../crates/tako-compat/Cargo.toml), [`Cargo.toml`](../Cargo.toml) |
| 37.C | `OidcAuthResolver::watch_mtls_provider(provider)` builder + `MtlsProviderWatcher` handle | [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs), [`crates/tako-compat/src/auth/mod.rs`](../crates/tako-compat/src/auth/mod.rs), [`crates/tako-compat/src/lib.rs`](../crates/tako-compat/src/lib.rs) |
| 37.D | Rust tests with a mock provider | inline `#[cfg(test)] mod tests` |
| 37.E | Recipe doc — when to use trait-based vs filesystem-watcher | [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) |
| 37.F | Workspace + Python version 0.37.0 → 0.38.0 | [`Cargo.toml`](../Cargo.toml), [`pyproject.toml`](../pyproject.toml), [`python/tako/__init__.py`](../python/tako/__init__.py), [`tests/python/test_smoke.py`](../tests/python/test_smoke.py) |
| 37.G | PLAN.md row + Phase 38 candidate-list refresh | [`PLAN.md`](../PLAN.md) |
| 37.H | CHANGELOG.md `[0.38.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 37.A — `MtlsIdentityProvider` trait

```rust
/// Phase 37 — async source of fresh mTLS cert+key bytes.
///
/// Implement this trait when filesystem-based rotation
/// ([`OidcAuthResolver::watch_mtls_files`], Phase 35) doesn't
/// fit your deployment shape — e.g. HSM-backed keys, in-memory
/// secret stores, on-demand fetch from a SPIFFE / AWS Roles
/// Anywhere broker.
///
/// Tako parses the returned cert's `NotAfter` and proactively
/// re-calls `fetch()` at 80% of the validity window. The
/// previously installed `MtlsClient` is preserved if a fetch
/// fails or returns malformed PEM.
#[async_trait]
pub trait MtlsIdentityProvider: Send + Sync + 'static + std::fmt::Debug {
    /// Return a fresh (cert, key) PEM pair. Called eagerly in
    /// the background once `watch_mtls_provider` is invoked,
    /// then periodically based on the cert's parsed expiry.
    async fn fetch(&self) -> Result<MtlsIdentity, TakoError>;
}

/// PEM-pair returned by [`MtlsIdentityProvider::fetch`].
#[derive(Clone, Debug)]
pub struct MtlsIdentity {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}
```

Both types are public exports. The PEM bytes are owned `Vec<u8>` so providers can return freshly-allocated bytes per call without lifetime entanglement.

### 37.B — Cert-expiry parsing

`x509-parser = "0.18"` added to workspace deps and to the
`tako-compat/mtls-identity-provider` feature gate. Default
`tako-compat` build is unchanged.

Internal helper:

```rust
/// Phase 37 — parse the leaf cert's `NotAfter` from a PEM blob.
/// Returns `None` if the cert is unparseable (e.g. operator
/// passed an opaque blob the trait pretended was a cert) — the
/// caller falls back to a fixed default-refresh interval.
fn parse_not_after(cert_pem: &[u8]) -> Option<SystemTime>;
```

Uses `x509_parser::pem::Pem::iter_from_buffer` for the PEM frame
walk and `x509_parser::parse_x509_certificate` for the DER
content. Multi-cert PEM blobs (cert chains) are handled by
parsing the first leaf only.

### 37.C — Builder + handle

```rust
impl OidcAuthResolver {
    /// Phase 37 — spawn a background task that periodically
    /// fetches a fresh mTLS identity from the operator-supplied
    /// [`MtlsIdentityProvider`] and reloads it via
    /// [`Self::reload_mtls_identity`]. The refresh schedule is
    /// driven by the returned cert's parsed `NotAfter`: tako
    /// sleeps until 80% of the validity window has elapsed,
    /// then re-fetches.
    ///
    /// The returned [`MtlsProviderWatcher`] handle owns the
    /// background task; dropping it (or calling
    /// [`MtlsProviderWatcher::shutdown`]) stops the watcher.
    /// Operators bind it to a module-scope variable for the
    /// lifetime of the resolver.
    pub fn watch_mtls_provider(
        self: Arc<Self>,
        provider: Arc<dyn MtlsIdentityProvider>,
    ) -> Result<MtlsProviderWatcher, TakoError>;
}

pub struct MtlsProviderWatcher { ... }
impl MtlsProviderWatcher {
    pub async fn shutdown(self);
}
```

Background task semantics:

- **Initial fetch** runs in the spawned task — does not block
  builder construction. The resolver was already configured
  with `with_introspection_mtls(initial_cert, initial_key)` so
  the running server has a valid identity until the first
  `provider.fetch().await` lands.
- **Refresh schedule:** parse `NotAfter`; sleep
  `(NotAfter - now) * 0.8`. Capped at 24h (so a cert with
  100-year validity doesn't sleep forever and miss
  out-of-band rotation signals).
- **Fetch errors:** `tracing::warn!` + retry after a 60s
  backoff. The previously installed Client stays in place per
  Phase 33 semantics.
- **Parse errors:** if `NotAfter` can't be parsed but
  `reload_mtls_identity` succeeded (the PEM is valid for
  rustls' purposes but x509-parser disagrees on something),
  fall back to a 1-hour default refresh interval. Logs at
  `warn` level so the operator notices.
- **Drop / shutdown:** signal `tokio::sync::Notify` to wake
  the task; abort the `JoinHandle`.

Constants:

```rust
const REFRESH_FRACTION: f64 = 0.8;
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);
const MAX_REFRESH_INTERVAL: Duration = Duration::from_secs(86400);
const ERROR_BACKOFF: Duration = Duration::from_secs(60);
```

### 37.D — Tests

Inline `#[cfg(test)] mod tests` in `oidc_mtls_provider.rs`:

1. `parse_not_after_extracts_expiry_from_test_cert` — feeds the existing test PEM through the parser, asserts a SystemTime in the future.
2. `parse_not_after_returns_none_on_garbage` — `b"not a cert"` → `None`.
3. `provider_fetch_drives_initial_reload` — counting mock provider returns the test cert; assert `MtlsClient::current()` Arc swaps within ~500ms of `watch_mtls_provider` returning.
4. `provider_fetch_error_preserves_client` — counting mock provider returns `Err`; assert no swap; assert provider is called more than once (retry happens).
5. `drop_stops_watcher` — drop the handle; assert no further fetches happen for 1s.
6. `errors_when_no_mtls_configured` — `watch_mtls_provider` on a resolver without prior `with_introspection_mtls` returns the same operator-friendly `TakoError::Invalid` as `watch_mtls_files`.

The tests use the same test PEM constants as the Phase 35 watcher tests (cert with ~100 year validity).

### 37.E — Python facade — DEFERRED to Phase 38

Implementing an async trait FROM Python via PyO3 is non-trivial — the operator would need to subclass and Python's GIL discipline complicates the async polling. Pure-Rust deployments (the typical `tako-compat` operator running their own Rust server) get the full Phase 37 surface today.

The Phase 38 candidate list will include "Python facade for MtlsIdentityProvider" as a follow-on, alongside the auto-refresh-on-handshake-failure layer.

### 37.F — Recipe doc

[`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) gets a "Trait-based identity provider (HSM, in-memory stores)" section between the existing filesystem-watcher (recommended) and hand-rolled (no-feature) sections.

### 37.G — Version bump + PLAN/CHANGELOG

Standard cadence — `0.37.0` → `0.38.0` across `Cargo.toml`, `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py`. PLAN.md row added; "Phase 38 candidates" replaces "Phase 37 candidates" (drops the trait-based item; adds the deferred Python-facade follow-on).

## Critical files

**Modified:**
- `Cargo.toml` — workspace `x509-parser` dep + version bump.
- `crates/tako-compat/Cargo.toml` — optional `x509-parser` dep + `mtls-identity-provider` feature.
- `crates/tako-compat/src/auth/mod.rs` — feature-gated `pub mod oidc_mtls_provider` re-export.
- `crates/tako-compat/src/auth/oidc.rs` — `watch_mtls_provider` builder.
- `crates/tako-compat/src/lib.rs` — re-export `MtlsIdentityProvider` / `MtlsIdentity` / `MtlsProviderWatcher`.
- `docs/recipes/mtls_rotation.md` — new section.
- `pyproject.toml` / `python/tako/__init__.py` / `tests/python/test_smoke.py` — version flip.
- `PLAN.md` / `CHANGELOG.md` — phase index + entry.

**Created:**
- `crates/tako-compat/src/auth/oidc_mtls_provider.rs` — trait + builder + tests.
- `plans/PLAN_PHASE37.md` (this file).

## Verification

1. `cargo fmt --all -- --check` passes.
2. `cargo clippy -p tako-compat --all-features -- -D warnings` passes.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings` passes.
4. `cargo test -p tako-compat --all-features` passes — the new `oidc_mtls_provider` test module adds ~6 tests.
5. `ruff format --check` + `ruff check` clean.
6. `pytest -q` passes (no Python-facade changes; just the version bump).

## Out of scope

- **Python facade for `MtlsIdentityProvider`.** Deferred to Phase 38+ because of Python-side async-trait ergonomics.
- **Automatic refresh-on-handshake-failure.** Still deferred; will sit on top of either the Phase 35 watcher or the Phase 37 provider. Phase 38 candidate.
- **Cert chain validation.** The trait yields opaque PEM bytes; tako parses only the leaf cert's `NotAfter` to schedule refresh. Chain validity is rustls' problem at handshake time.
- **Custom refresh-interval policies.** Phase 37 hard-codes 80% of validity. Operators with niche requirements can implement their own provider that adjusts cert validity at fetch time, or can wrap their refresh logic externally and call `reload_mtls_identity` directly.
