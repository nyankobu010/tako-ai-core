# PLAN — Phase 43 (Python facade for `_extra_root` mTLS introspection builders)

> **Status: in progress.** Targets v0.44.0. Carry-forward from
> [Phase 42](PLAN_PHASE42.md) — closes the Python facade
> deferred when Phase 42 shipped the Rust-side builders.

## Context

Phase 42 (v0.43.0) shipped Rust-only:

- `OidcAuthResolver::with_introspection_mtls_extra_root(cert_pem,
  key_pem, extra_root_ca_pem)` — RFC 8705 `tls_client_auth`
  variant that adds an operator-supplied PEM-encoded root CA
  bundle to the underlying `reqwest::Client`'s trust store.
- `OidcAuthResolver::with_introspection_self_signed_mtls_extra_root(...)`
  — RFC 8705 §2.2 sibling, identical wire shape, only the
  `auth_method` enum variant differs
  (`SelfSignedTlsClientAuth`).
- `IntrospectionConfig::extra_root_ca_pem: Option<Arc<Vec<u8>>>`
  — public field that persists the bundle so the rotation
  surfaces (Phase 33 `reload_mtls_identity`, Phase 35
  `MtlsFsWatcher`, Phase 37 `MtlsProviderWatcher`, Phase 39
  refresh hook) re-apply the same trust anchors when
  rebuilding the mTLS client after a cert/key swap.
- Five new wire-level integration tests in
  `crates/tako-compat/tests/oidc_mtls_e2e.rs` covering
  happy-path, missing-CA-rejected, missing-client-cert-rejected,
  self-signed happy-path, and unparseable-PEM at builder time.

Python-wheel operators have **no current path** to use this
surface. The matching `with_introspection_mtls` /
`with_introspection_self_signed_mtls` Python builders shipped
in Phase 24.B / 25.B but their `_extra_root` siblings are
absent. Operators behind a private internal CA (Keycloak
self-hosted, Auth0 self-hosted, Authentik) currently can't
introspect from Python without falling back to a custom
`reqwest::Client` route — which means leaving the Python wheel
entirely.

Bridging is mechanical: two new pyclass methods on `PyOidcAuth`,
one-line `Result` map for each, no new pyclass, no new
registration, no new feature gate (the existing `auth-oidc`
gate covers the parent `OidcAuth`). The `extra_root_ca_pem`
config field is read-only operator data so doesn't need to be
exposed independently.

## Why now

Phase 42 closed the wire-level test gap on this surface. The
Rust API is feature-complete on the operator-CA story for
introspection. Closing the Python facade puts wheel operators
at parity with Rust on the **entire** Phase 24 / 25 / 33 / 35 /
37 / 39 / 42 mTLS introspection pipeline. Same cadence as
Phase 38 (Python facade for the Phase 37 trait-based provider)
and Phase 40 (Python facade for the Phase 39 refresh hook):
ship the Rust primitive in one phase, wrap it for Python in
the next.

This phase has no new design decisions; the Rust API is the
contract.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 43.A | `PyOidcAuth.with_introspection_mtls_extra_root(cert_pem, key_pem, extra_root_ca_pem)` | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) |
| 43.B | `PyOidcAuth.with_introspection_self_signed_mtls_extra_root(cert_pem, key_pem, extra_root_ca_pem)` | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) |
| 43.C | Docstring update on `python/tako/compat.py` documenting the new builders alongside the existing Phase 24 / 25 prose | [`python/tako/compat.py`](../python/tako/compat.py) |
| 43.D | Python smoke test pinning the binding | [`tests/python/test_phase43_mtls_extra_root_python.py`](../tests/python/test_phase43_mtls_extra_root_python.py) |
| 43.E | Workspace + Python version 0.43.0 → 0.44.0 | various |
| 43.F | PLAN.md row + Phase 44 candidate refresh | [`PLAN.md`](../PLAN.md) |
| 43.G | CHANGELOG.md `[0.44.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

No new pyclass: the `IntrospectionConfig::extra_root_ca_pem`
field is config data the resolver carries internally — Python
operators don't construct or read it.

No new feature gate: the existing `auth-oidc` gate covers the
parent `PyOidcAuth`. Both new methods sit under the same
`#[cfg(feature = "auth-oidc")]` block.

## What this phase will land

### 43.A + 43.B — `_extra_root` builders on `PyOidcAuth`

In `crates/tako-py/src/py_compat.rs`, immediately after
`with_introspection_self_signed_mtls_combined` (line ~515) and
before the Phase 33 rotation methods:

```rust
/// Phase 42 — load a client cert + private key from separate
/// PEM blobs AND add an operator-supplied PEM-encoded root CA
/// bundle to the underlying HTTP client's trust store. Same
/// behaviour as
/// [`Self::with_introspection_mtls`] otherwise (silent no-op
/// when no introspection config has been attached, PEM parse
/// failures map to `ValueError`). Use this for enterprise
/// self-hosted OIDC issuers (Keycloak / Auth0 self-hosted /
/// Authentik) presenting a server cert signed by a private
/// internal CA.
///
/// `extra_root_ca_pem` accepts a single root cert or a
/// concatenated multi-cert PEM bundle. The CA bundle is
/// persisted on the introspection config so subsequent
/// `reload_mtls_identity` calls (and the rotation surfaces in
/// Phases 35 / 37 / 39 that route through them) re-apply the
/// same trust anchors.
fn with_introspection_mtls_extra_root(
    &self,
    cert_pem: &[u8],
    key_pem: &[u8],
    extra_root_ca_pem: &[u8],
) -> PyResult<Self> {
    let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
    let r = cloned
        .with_introspection_mtls_extra_root(cert_pem, key_pem, extra_root_ca_pem)
        .map_err(map_err)?;
    Ok(PyOidcAuth { inner: Arc::new(r) })
}

/// Phase 42 — same as
/// [`Self::with_introspection_mtls_extra_root`] but with the
/// RFC 8705 §2.2 `self_signed_tls_client_auth` auth method.
/// Identical wire shape; the only difference is the auth-
/// method enum variant.
fn with_introspection_self_signed_mtls_extra_root(
    &self,
    cert_pem: &[u8],
    key_pem: &[u8],
    extra_root_ca_pem: &[u8],
) -> PyResult<Self> {
    let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
    let r = cloned
        .with_introspection_self_signed_mtls_extra_root(cert_pem, key_pem, extra_root_ca_pem)
        .map_err(map_err)?;
    Ok(PyOidcAuth { inner: Arc::new(r) })
}
```

Both follow the exact pattern of
`with_introspection_mtls` / `with_introspection_self_signed_mtls`
above them: clone the inner `Arc<OidcAuthResolver>`, call the
matching Rust builder, wrap in a fresh `PyOidcAuth`. Returns
a NEW `OidcAuth` (immutable builder, matches the Phase 14
cadence). Errors surface as `ValueError` via the same
`map_err` helper that's been the standard since Phase 14.

The Phase 24 `_combined` cadence stays out of scope here for
the same reason as in Phase 42 — operators with combined
PEMs can pass the same byte slice for cert / key, and adding
a `_combined` shape across an additional CA argument doesn't
generalize cleanly. Defer until ask.

### 43.C — Python facade docstring

Append a Phase 42 paragraph to the `serve_openai` docstring's
running list, immediately after the Phase 33.B paragraph
(line ~108):

```python
"""
Phase 42 — ``OidcAuth.with_introspection_mtls_extra_root(cert_pem, key_pem, extra_root_ca_pem)``
and ``with_introspection_self_signed_mtls_extra_root(...)``
are the operator-supplied-CA siblings of the Phase 24 / 25
mTLS introspection builders. The CA bundle (single root or
concatenated multi-cert PEM) is added to the underlying
HTTP client's trust store and is persisted across the
Phase 33 / 35 / 37 / 39 rotation surfaces. For enterprise
self-hosted OIDC issuers (Keycloak / Auth0 self-hosted /
Authentik) presenting a server cert signed by a private
internal CA. Raises ``ValueError`` on PEM parse failure
(empty bundle, garbage bytes) at builder time — fail-closed.
"""
```

No new module-level re-export needed; the `OidcAuth` symbol
already exposes the new methods.

### 43.D — Python smoke test

`tests/python/test_phase43_mtls_extra_root_python.py`:

1. `OidcAuth.with_introspection_mtls_extra_root` is callable
   when the wheel was built with `auth-oidc`.
2. `OidcAuth.with_introspection_self_signed_mtls_extra_root`
   is callable when the wheel was built with `auth-oidc`.
3. Both methods skip themselves on slim wheels (the
   `auth-oidc` gate covers the entire `OidcAuth` pyclass, so
   the skip key is `compat.OidcAuth is None`).

Test contents pin the binding so a regression in the PyO3
wrapping (wrong arg count, wrong arg type, missing
`map_err`) lands here before user code. The actual
PEM-parsing / wire-level semantics are covered by the
Rust unit + e2e tests in Phase 42; the Python test asserts
*existence* and *callability* — same shape as
[`tests/python/test_phase40_mtls_refresh_hook_python.py`](../tests/python/test_phase40_mtls_refresh_hook_python.py).

### 43.E — Version bump

0.43.0 → 0.44.0 across `Cargo.toml`, `pyproject.toml`,
`python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 43.F — PLAN.md update

- New row `43 — Python facade for `_extra_root` mTLS
  introspection builders`.
- Phase 44 candidate list: surviving items from Phase 42's
  out-of-scope section (custom CA support for non-introspection
  endpoints, eval-harness real graders, OTel real-collector
  e2e, Vertex deterministic-per-call placeholder logic) plus
  any new mTLS asks. None of these are pre-committed.

### 43.G — CHANGELOG `[0.44.0]`

Standard format, brief — closes the Python parity gap.

## Critical files

**Modified:**
- [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) — two new methods.
- [`python/tako/compat.py`](../python/tako/compat.py) — docstring paragraph.
- Standard PLAN/CHANGELOG/version flip:
  - [`Cargo.toml`](../Cargo.toml)
  - [`pyproject.toml`](../pyproject.toml)
  - [`python/tako/__init__.py`](../python/tako/__init__.py)
  - [`tests/python/test_smoke.py`](../tests/python/test_smoke.py)
  - [`PLAN.md`](../PLAN.md)
  - [`CHANGELOG.md`](../CHANGELOG.md)
- `Cargo.lock` (version-bump fallout only).

**Created:**
- [`tests/python/test_phase43_mtls_extra_root_python.py`](../tests/python/test_phase43_mtls_extra_root_python.py)
- [`plans/PLAN_PHASE43.md`](PLAN_PHASE43.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy -p tako-py --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider" -- -D warnings`.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
4. `cargo test --workspace --exclude tako-py --all-features` — no regressions.
5. `ruff format --check python/ tests/` + `ruff check python/ tests/`.
6. `maturin develop --release --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider"` — wheel builds at v0.44.0.
7. `pytest -q tests/python/test_phase43_mtls_extra_root_python.py tests/python/test_smoke.py` — new test passes, smoke pins v0.44.0.
8. `pytest -q` — full suite green.

## Out of scope

- **`_combined` PEM siblings for the new builders.** Same
  rationale as Phase 42 (Rust side) — operators with
  combined PEMs can pass the same byte slice twice. Defer
  until an explicit ask.
- **Custom CA support for non-introspection endpoints (JWKS,
  discovery).** Same as Phase 42 out-of-scope. Touches
  `discover()` boot path; separate phase.
- **Exposing `IntrospectionConfig::extra_root_ca_pem` to
  Python readers.** It's internal config; operators don't
  need to introspect it. The persistence-across-rotation
  invariant is covered by Rust unit tests in Phase 42.
- **`MtlsRefreshHook.force_refresh()` from Python.** Same as
  Phase 40 out-of-scope.
