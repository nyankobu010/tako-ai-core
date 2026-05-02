# PLAN — Phase 33 (OIDC mTLS cert/key rotation)

## Context

Phases 24 (`tls_client_auth`) and 25 (`self_signed_tls_client_auth`)
shipped mTLS for the OIDC introspection endpoint. The
`OidcAuthResolver::with_introspection_mtls(cert_pem, key_pem)`
builder loads the cert + key once at builder time and caches the
resulting `reqwest::Client` (with `reqwest::Identity` attached)
on `IntrospectionConfig.mtls_client`. Every introspection POST
reuses that pre-built Client.

Production deployments using cert-manager, Vault PKI, SPIRE, or
any other PKI rotation tool refresh client certs daily / weekly
/ on-demand. The Phase 24/25 surface forces a process restart
to pick up new certs — which is unacceptable for long-running
tako-compat servers.

Phase 33 closes that gap with an explicit-reload primitive:
operators call `OidcAuthResolver::reload_mtls_identity(cert_pem,
key_pem)` from their own scheduler (cert-manager webhook,
filesystem watcher, periodic poll) and the next request uses the
new identity. The reload is atomic from the request-handler's
perspective — concurrent requests either see the old Client or
the new one, never a torn state.

## Why explicit-reload instead of automatic expiry-based refresh

Three options for rotation, in increasing complexity:

1. **Explicit operator-controlled reload** — operator's
   responsibility to schedule reload before expiry. Tako
   provides only the swap primitive. Simplest; matches how
   most cert-rotation tooling already works (cert-manager
   updates files; operator's app picks them up).
2. **Trait-based identity provider** — operator implements an
   `MtlsIdentityProvider` async trait that yields fresh
   cert+key bytes; tako re-fetches when expiry approaches.
   More structured but needs cert-parsing on the tako side.
3. **Automatic refresh-on-handshake-failure** — tako catches
   TLS handshake errors at request time and triggers reload.
   Requires retry logic + cycle-detection.

Phase 33 ships only (1). Future phases can add (2) and (3) if
real demand surfaces. (1) covers the vast majority of mTLS
rotation scenarios with the smallest tako-side surface area.

## A. Rust core: `MtlsClient` newtype + reload method

### A.1 — New private `MtlsClient` newtype

[`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs):

```rust
/// Phase 33 — swap-able holder for the mTLS-enabled
/// `reqwest::Client` used by the OIDC introspection endpoint.
/// Built by [`OidcAuthResolver::with_introspection_mtls`] /
/// [`OidcAuthResolver::with_introspection_self_signed_mtls`];
/// swapped at runtime by
/// [`OidcAuthResolver::reload_mtls_identity`] when operator
/// rotates the underlying cert+key (cert-manager webhook,
/// Vault PKI rotation, etc).
///
/// The swap is atomic from the request-handler's perspective:
/// concurrent introspection POSTs either see the old Client
/// or the new one, never a torn state.
#[derive(Debug)]
pub struct MtlsClient {
    inner: std::sync::RwLock<Arc<reqwest::Client>>,
}

impl MtlsClient {
    pub(crate) fn new(client: reqwest::Client) -> Self {
        Self {
            inner: std::sync::RwLock::new(Arc::new(client)),
        }
    }

    /// Snapshot the current Client for one request.
    pub(crate) fn current(&self) -> Arc<reqwest::Client> {
        match self.inner.read() {
            Ok(guard) => guard.clone(),
            // Poisoned: another thread panicked while holding
            // the write lock. The data is still valid; just
            // the lock state is dirty. Recover by reading
            // through the poison.
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Atomically replace the inner Client.
    pub(crate) fn swap(&self, client: reqwest::Client) {
        let new_arc = Arc::new(client);
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = new_arc;
    }
}
```

### A.2 — Field type change on `IntrospectionConfig`

```rust
// Phase 24 (was):
pub mtls_client: Option<Arc<reqwest::Client>>,

// Phase 33 (new):
pub mtls_client: Option<Arc<MtlsClient>>,
```

This is a public-API change: external callers who construct
`IntrospectionConfig` directly with a literal
`mtls_client: None` field still work (`None` is a valid value
under both types); callers who passed `Some(Arc::new(client))`
need to use `Some(Arc::new(MtlsClient::new(client)))`. The
struct is barely 6 weeks old (shipped in Phase 24); no
external callers are expected.

### A.3 — `with_introspection_mtls` + `with_introspection_self_signed_mtls` builders

Both swap their `cfg.mtls_client = Some(Arc::new(client))` to
`cfg.mtls_client = Some(Arc::new(MtlsClient::new(client)))`.
Otherwise unchanged. Public surface preserved byte-for-byte.

### A.4 — Read path in `introspect()`

```rust
// Phase 24 (was):
let http: &reqwest::Client = match cfg.auth_method {
    IntrospectionAuthMethod::TlsClientAuth
    | IntrospectionAuthMethod::SelfSignedTlsClientAuth => cfg
        .mtls_client
        .as_deref()
        .unwrap_or(&self.http),
    _ => &self.http,
};
// (later: req.send().await ...)

// Phase 33 (new):
let mtls_snapshot: Option<Arc<reqwest::Client>> = match cfg.auth_method {
    IntrospectionAuthMethod::TlsClientAuth
    | IntrospectionAuthMethod::SelfSignedTlsClientAuth => {
        cfg.mtls_client.as_ref().map(|m| m.current())
    }
    _ => None,
};
let http: &reqwest::Client = mtls_snapshot.as_deref().unwrap_or(&self.http);
// (later: req.send().await ...)
```

The snapshot lives for the duration of the request; concurrent
swaps after the snapshot is taken don't affect the in-flight
request.

### A.5 — New `OidcAuthResolver::reload_mtls_identity()` method

```rust
/// Phase 33 — atomically replace the mTLS identity used for
/// OIDC introspection POSTs. Useful for cert rotation in
/// long-running deployments (cert-manager webhook, Vault PKI
/// rotation, filesystem watcher, etc).
///
/// Errors when no introspection mTLS config has been attached
/// (no prior `with_introspection_mtls` /
/// `with_introspection_self_signed_mtls` call) — operators
/// notice early rather than silent-no-op.
///
/// PEM parse / `reqwest::Client` build failures surface as
/// `TakoError::Invalid` and leave the existing Client
/// unchanged.
pub fn reload_mtls_identity(
    &self,
    cert_pem: &[u8],
    key_pem: &[u8],
) -> Result<(), TakoError> {
    let holder = self
        .introspection
        .as_ref()
        .and_then(|cfg| cfg.mtls_client.as_ref())
        .ok_or_else(|| {
            TakoError::Invalid(
                "oidc: reload_mtls_identity called but no mTLS identity configured \
                 (call with_introspection_mtls or with_introspection_self_signed_mtls first)"
                    .into(),
            )
        })?;
    let identity = build_mtls_identity(cert_pem, key_pem)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .identity(identity)
        .build()
        .map_err(|e| {
            TakoError::Invalid(format!("oidc: failed to build mTLS client: {e}"))
        })?;
    holder.swap(client);
    Ok(())
}

/// Phase 33 — convenience for combined PEM blobs (matches
/// the Phase 24 `with_introspection_mtls_combined` cadence).
pub fn reload_mtls_identity_combined(
    &self,
    combined_pem: &[u8],
) -> Result<(), TakoError> {
    self.reload_mtls_identity(combined_pem, combined_pem)
}
```

Note `&self` not `&mut self` — internal mutability lives on
the `MtlsClient::inner` RwLock. This lets operators call
`reload_mtls_identity` through an `Arc<OidcAuthResolver>`
without needing exclusive access.

### A.6 — Tests

Unit tests in `oidc.rs`:
- `mtls_client_current_returns_swappable_arc` — `MtlsClient::new`
  followed by two `current()` calls returns Arcs that point at
  the same inner Client (via `Arc::ptr_eq`).
- `mtls_client_swap_replaces_inner` — after `swap()`, `current()`
  returns an Arc pointing to the NEW Client (not the old one).
- `reload_mtls_identity_swaps_under_arc_resolver` — wrap the
  resolver in `Arc`, call reload, verify that introspection
  config's holder now serves a different Arc.
- `reload_mtls_identity_errs_when_no_mtls_configured` —
  resolver without prior `with_introspection_mtls` call returns
  `TakoError::Invalid` from `reload_mtls_identity` (operator
  error rather than silent no-op).
- `reload_mtls_identity_errs_on_invalid_pem_and_preserves_old`
  — feed garbage PEM; verify Err returned AND the previously
  installed Client is still served by `current()` (no
  partial-rollback: invalid input doesn't taint the cache).
- `reload_mtls_identity_combined_works_for_combined_pem` —
  `cat cert.pem key.pem` form roundtrips through the combined
  helper.
- `reload_mtls_identity_works_for_self_signed_too` — set up
  with `with_introspection_self_signed_mtls`; reload works
  identically.

Phase 24 + 25 tests pass byte-for-byte unchanged (the public
builder + introspect surface is preserved; only the field type
on `IntrospectionConfig` changes).

## B. Python facade: `OidcAuth.reload_mtls_identity()`

### B.1 — `crates/tako-py/src/py_compat.rs`

Find the `PyOidcAuth` pyclass. Add new methods:

```rust
/// Phase 33 — reload the mTLS identity used for OIDC
/// introspection. Mirrors
/// [`OidcAuthResolver::reload_mtls_identity`].
fn reload_mtls_identity(
    &self,
    cert_pem: &[u8],
    key_pem: &[u8],
) -> PyResult<()> {
    self.inner
        .reload_mtls_identity(cert_pem, key_pem)
        .map_err(map_err)
}

fn reload_mtls_identity_combined(
    &self,
    combined_pem: &[u8],
) -> PyResult<()> {
    self.reload_mtls_identity(combined_pem, combined_pem)
}
```

Note `&self` — unlike the `with_*` builders (which return a
new `OidcAuth` via the immutable-builder pattern), reload
mutates state in place via internal mutability. This matches
the operator workflow: take an existing `OidcAuth` reference
and call reload on it.

### B.2 — Python-side mirror

[`python/tako/compat.py`](python/tako/compat.py): document
the new methods on the `OidcAuth` class docstring. The methods
are reachable via `__getattr__` delegation to the underlying
PyO3 pyclass.

### B.3 — Tests

[`tests/python/test_phase33_oidc_mtls_reload.py`](tests/python/test_phase33_oidc_mtls_reload.py)
(NEW) — signature smoke + roundtrip test:
- `OidcAuth.reload_mtls_identity` method exists and accepts
  bytes args.
- `OidcAuth.reload_mtls_identity_combined` method exists and
  accepts a single bytes arg.
- Calling reload on an OidcAuth with no prior mTLS config
  raises (mirrors the Rust-side error).
- Calling reload after `with_introspection_mtls` succeeds and
  doesn't raise.

The tests use the same RSA test fixtures already embedded in
`oidc.rs` for Phase 17/18 — no live TLS handshake needed.

## Out of scope (deferred to Phase 34+)

- **Trait-based `MtlsIdentityProvider`** — async trait that
  yields fresh cert+key bytes on demand; tako would call it
  proactively at e.g. 90% of cert validity. Needs cert-parsing
  on the tako side (`x509-parser` dep or hand-rolled DER walk).
- **Automatic refresh-on-handshake-failure** — catch TLS
  handshake errors at request time and trigger reload. Needs
  retry logic + cycle-detection.
- **Filesystem watcher integration** — auto-reload when the
  cert+key files on disk change. `notify` crate dep.
- **OIDC refresh-token / revocation-endpoint flows** —
  separate concern from mTLS rotation.
- **Vertex File API upload flow** (Phase 23 carry-forward).
- **`TakoError::Provider` short-circuit on
  `ChainedAuthResolver`** (Phase 27 carry-forward).

## Acceptance criteria

- `cargo test -p tako-compat --features oidc` passes
- `cargo test -p tako-py --all-features` passes
- `cargo clippy --workspace --all-features -- -D warnings` passes
- `cargo fmt --all -- --check` passes
- `pytest tests/python/test_phase33_oidc_mtls_reload.py` passes
  (after `maturin develop --release --features auth-oidc`)
- `pytest -q` passes (no regressions)

## Commit cadence

1. `docs: PLAN_PHASE33.md`
2. `feat(tako-compat): OIDC mTLS cert/key rotation (Phase 33.A)`
3. `feat(tako-py): OidcAuth.reload_mtls_identity Python facade (Phase 33.B)`
4. `docs: Phase 33 PLAN/README/CHANGELOG flip (v0.34.0)`
