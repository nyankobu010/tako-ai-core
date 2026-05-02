# PLAN — Phase 45 (Python facade for `discover_with_extra_root`)

> **Status: in progress.** Targets v0.46.0. Carry-forward from
> [Phase 44](PLAN_PHASE44.md) — closes the Python facade
> deferred when Phase 44 shipped the Rust-side constructor.

## Context

Phase 44 (v0.45.0) shipped Rust-only:

- `OidcAuthResolver::discover_with_extra_root(issuer, audience, extra_root_ca_pem)`
  — parallel async constructor that builds the resolver-wide
  `reqwest::Client` with an operator-supplied PEM-encoded root
  CA bundle added to its trust store. The same trust anchor
  covers BOTH the OIDC discovery doc fetch AND every subsequent
  JWKS refresh, because the resolver holds a single `http`
  field for non-introspection HTTP.
- Internal `build_resolver_http_client(extra_root_ca_pem)`
  helper.
- 3 lib unit tests + 3 e2e tests (HTTPS-with-private-CA
  `axum-server`).

Python wheel operators behind a private internal CA still
have **no path** to use this. Today they hit
`OidcAuth.discover(issuer, audience)` and the GraphQL fails
at TLS verification before the resolver returns. After
Phase 44 there's a Rust escape hatch, but the Python facade
exposes only the public-CA `discover` constructor.

Bridging is mechanical: one new pyclass `#[staticmethod]`,
no new pyclass, no new feature gate (the existing `auth-oidc`
gate covers the parent `PyOidcAuth`). Same shape as
[`PyOidcAuth::discover`](../crates/tako-py/src/py_compat.rs)
— wraps the future via `pyo3_async_runtimes::tokio::future_into_py`.

## Why now

This is the bookend on the Phase 44 thread. Closing it puts
Python wheel operators at parity with Rust on the **entire**
Phase 24 / 25 / 33 / 35 / 37 / 39 / 42 / 43 / 44 mTLS + OIDC
private-CA story. Same cadence as Phases 38 (Python facade
for the Phase 37 trait-based provider) and 40 (Python facade
for the Phase 39 refresh hook) and 43 (Python facade for the
Phase 42 introspection-mTLS `_extra_root` builders): ship the
Rust primitive in one phase, wrap it for Python in the next.

This phase has zero new design decisions; the Rust API is
the contract.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 45.A | `PyOidcAuth.discover_with_extra_root(issuer, audience, extra_root_ca_pem)` async staticmethod | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) |
| 45.B | Docstring update on `python/tako/compat.py` | [`python/tako/compat.py`](../python/tako/compat.py) |
| 45.C | Python smoke test pinning the binding | [`tests/python/test_phase45_discover_extra_root_python.py`](../tests/python/test_phase45_discover_extra_root_python.py) |
| 45.D | Workspace + Python version 0.45.0 → 0.46.0 | various |
| 45.E | PLAN.md row + Phase 46 candidate refresh | [`PLAN.md`](../PLAN.md) |
| 45.F | CHANGELOG.md `[0.46.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 45.A — `discover_with_extra_root` Python staticmethod

Immediately after the existing `discover` staticmethod
(line ~316 in `py_compat.rs`):

```rust
/// Phase 44 — async constructor that builds the
/// resolver-wide HTTP client with an operator-supplied
/// PEM-encoded root CA bundle added to its trust store.
/// Use this for enterprise self-hosted OIDC issuers
/// (Keycloak / Auth0 self-hosted / Authentik) presenting
/// a server cert signed by a private internal CA — without
/// it, the discovery GET fails TLS verification before the
/// resolver is even returned.
///
/// `extra_root_ca_pem` accepts a single root cert or a
/// concatenated multi-cert PEM bundle. PEM parse failures
/// (empty bundle, garbage bytes) raise `ValueError` at
/// construction time — fail-closed at the operator
/// boundary.
///
/// Independent from `with_introspection_mtls_extra_root`:
/// the introspection mTLS client carries its own CA store.
/// Operators with one PKI for the whole stack pass the
/// same PEM bundle to both.
#[staticmethod]
fn discover_with_extra_root<'py>(
    py: Python<'py>,
    issuer: String,
    audience: String,
    extra_root_ca_pem: Vec<u8>,
) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let r = tako_compat::OidcAuthResolver::discover_with_extra_root(
            &issuer,
            &audience,
            &extra_root_ca_pem,
        )
        .await
        .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    })
}
```

The `Vec<u8>` arg shape matches the existing
`with_introspection_mtls` family (Phase 24.B and onward) so
Python callers pass `bytes` exactly the same way they do
today. We move ownership into the async block so the borrow
satisfies the `'static` future bound — same pattern as
`discover`.

No `#[pyo3(signature = ...)]` is needed; all three args are
required positionally.

### 45.B — `python/tako/compat.py` docstring

Append a Phase 44 paragraph to the running list,
immediately after the Phase 42 paragraph:

```python
"""
Phase 44 — ``OidcAuth.discover_with_extra_root(issuer, audience, ca_pem)``
is a parallel async constructor that builds the
resolver-wide HTTP client with an operator-supplied
PEM-encoded root CA bundle added to its trust store. The
same trust anchor covers BOTH the OIDC discovery doc
fetch (during construction) AND every subsequent JWKS
refresh, because the resolver holds a single ``http``
field for non-introspection HTTP. For enterprise
self-hosted OIDC issuers (Keycloak / Auth0 self-hosted /
Authentik) presenting a server cert signed by a private
internal CA. Independent from
``with_introspection_mtls_extra_root`` — operators with
one PKI for the whole stack pass the same PEM bundle to
both. Raises ``ValueError`` at construction time on PEM
parse failure (empty bundle, garbage bytes).
"""
```

No new module-level re-export needed; the `OidcAuth`
symbol already exposes the new staticmethod.

### 45.C — Python smoke test

`tests/python/test_phase45_discover_extra_root_python.py`:

1. `OidcAuth.discover_with_extra_root` exists when the
   wheel is built with `auth-oidc`.
2. It's a static method (callable without an instance).
3. Tests skip themselves on slim wheels (`compat.OidcAuth is None`).
4. The returned object from `discover_with_extra_root` is
   awaitable (i.e. wrapping `future_into_py` works); we
   don't actually await it (would need a network call).

The wire-level / PEM-parse semantics are covered by the
Rust unit + e2e tests in Phase 44; the Python test pins
the binding shape — same shape as
[`tests/python/test_phase43_mtls_extra_root_python.py`](../tests/python/test_phase43_mtls_extra_root_python.py).

### 45.D — Version bump

0.45.0 → 0.46.0 across `Cargo.toml` (workspace + 14
internal crate version pins), `pyproject.toml`,
`python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 45.E — PLAN.md update

- New row `45 — Python facade for discover_with_extra_root`.
- Phase 46 candidate list — surviving items: eval-harness
  real graders, OTel real-collector e2e, Vertex
  deterministic-per-call placeholder logic, OIDC
  refresh-token / revocation. None pre-committed.

### 45.F — CHANGELOG `[0.46.0]`

Standard format, brief — closes the Python parity gap.

## Critical files

**Modified:**
- [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs)
  — one new staticmethod.
- [`python/tako/compat.py`](../python/tako/compat.py)
  — docstring paragraph.
- Standard PLAN/CHANGELOG/version flip:
  [`Cargo.toml`](../Cargo.toml),
  [`pyproject.toml`](../pyproject.toml),
  [`python/tako/__init__.py`](../python/tako/__init__.py),
  [`tests/python/test_smoke.py`](../tests/python/test_smoke.py),
  [`PLAN.md`](../PLAN.md),
  [`CHANGELOG.md`](../CHANGELOG.md).
- `Cargo.lock` (version-bump fallout only).

**Created:**
- [`tests/python/test_phase45_discover_extra_root_python.py`](../tests/python/test_phase45_discover_extra_root_python.py).
- [`plans/PLAN_PHASE45.md`](PLAN_PHASE45.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy -p tako-py --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider" -- -D warnings`.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
4. `cargo test --workspace --exclude tako-py --all-features` — no regressions.
5. `ruff format --check python/ tests/` + `ruff check python/ tests/`.
6. `maturin develop --release --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider sigstore"` — wheel builds at v0.46.0.
7. `pytest -q tests/python/test_phase45_discover_extra_root_python.py tests/python/test_smoke.py` — new test passes, smoke pins v0.46.0.
8. `pytest -q` — full suite green.

## Out of scope

- **Sync / non-async Python sibling.** All `OidcAuth`
  constructors are async because discovery requires
  network I/O. Operators who need a sync path can wrap the
  call in `asyncio.run`. Adding a `discover_sync_with_extra_root`
  matches no existing precedent in the facade.
- **Filesystem-watcher / Vault-PKI integration for the
  resolver-wide CA bundle.** Phase 35 watches the
  introspection mTLS cert/key; the discovery / JWKS path
  has different rotation semantics (private CA roots
  rotate on the order of years). Defer until ask.
- **Exposing the CA bytes back through Python** (e.g.
  `OidcAuth.extra_root_ca_pem` getter). It's internal
  bootstrap data; Python operators don't read it back.
