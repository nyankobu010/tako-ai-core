# PLAN — Phase 38 (Python facade for `MtlsIdentityProvider`)

> **Status: in progress.** Targets v0.39.0. Carry-forward from
> [Phase 37](PLAN_PHASE37.md) — closes the deferred Python
> facade for the trait-based mTLS identity provider.

## Context

Phase 37 (v0.38.0) shipped Rust-only:

- `MtlsIdentityProvider` async trait whose `fetch()` yields
  `MtlsIdentity { cert_pem, key_pem }`.
- `OidcAuthResolver::watch_mtls_provider(provider)` builder
  spawning a background task that re-fetches at 80% of the
  cert's parsed validity window.

Python-wheel operators couldn't use it. The Phase 37 plan
flagged the deferral as "PyO3 async-trait subclassing
ergonomics need design"; this phase ships the design.

The pattern that works: a Python callable bridge (matches the
existing `PyPythonProvider` for the `LlmProvider` trait —
[`crates/tako-py/src/py_python_provider.rs`](../crates/tako-py/src/py_python_provider.rs)).
Operator passes an `async def fetch() -> tuple[bytes, bytes]`
(or a dict-returning coroutine); the Rust impl marshals via
`pyo3_async_runtimes::tokio::into_future`. No Python
inheritance / subclassing required.

## Why now

Phase 37 left this as the natural follow-on. After Phase 38,
Python-wheel operators get full parity with the Rust API for
mTLS identity providers — HSM-backed keys, in-memory secret
stores, SPIFFE Workload API, AWS IAM Roles Anywhere are all
expressible through the wheel.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 38.A | `PyMtlsIdentityProvider` pyclass wrapping a Python async callable | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) |
| 38.B | Wheel feature `auth-mtls-identity-provider` forwarding to `tako-compat/mtls-identity-provider` | [`crates/tako-py/Cargo.toml`](../crates/tako-py/Cargo.toml) |
| 38.C | `PyOidcAuth.watch_mtls_provider(provider)` Python method + `PyMtlsProviderWatcher` handle | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs), [`crates/tako-py/src/lib.rs`](../crates/tako-py/src/lib.rs) |
| 38.D | Re-export `MtlsIdentityProvider` / `MtlsProviderWatcher` from `tako.compat` | [`python/tako/compat.py`](../python/tako/compat.py) |
| 38.E | Recipe doc — Python example for the trait-based provider | [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) |
| 38.F | Python smoke test | [`tests/python/test_phase38_mtls_provider_python.py`](../tests/python/test_phase38_mtls_provider_python.py) |
| 38.G | Workspace + Python version 0.38.0 → 0.39.0 | various |
| 38.H | PLAN.md row + Phase 39 candidate refresh | [`PLAN.md`](../PLAN.md) |
| 38.I | CHANGELOG.md `[0.39.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 38.A — `PyMtlsIdentityProvider` pyclass

Modeled on `PyPythonProvider`. Operator constructs:

```python
async def fetch():
    cert, key = await my_hsm.issue_cert()
    return cert, key   # tuple of bytes

provider = tako.compat.MtlsIdentityProvider(fetch)
```

Internal Rust impl:

```rust
#[derive(Debug)]
struct PyMtlsImpl {
    fetch_callable: Py<PyAny>,
}

#[async_trait]
impl tako_compat::MtlsIdentityProvider for PyMtlsImpl {
    async fn fetch(&self) -> Result<MtlsIdentity, TakoError> {
        // 1. Python::attach — call the Python coroutine, get
        //    a future via pyo3_async_runtimes::tokio::into_future.
        // 2. Drop GIL; await the future.
        // 3. Python::attach — extract (cert_bytes, key_bytes)
        //    from a tuple, or {"cert_pem": ..., "key_pem": ...}
        //    from a dict. Both shapes are accepted.
    }
}
```

Both shapes are accepted to match operator preference; the
dict form is more discoverable, the tuple form is terser.
Other return types raise `TakoError::Invalid` with a clear
diagnostic.

### 38.B — Wheel feature

```toml
# Phase 38 — wheel-side feature gate for the trait-based mTLS
# identity provider (Phase 37 Rust API). Adds `x509-parser` for
# cert NotAfter parsing.
auth-mtls-identity-provider = ["auth-oidc", "tako-compat/mtls-identity-provider"]
```

Implies `auth-oidc` (the trait only makes sense with mTLS
introspection wired up).

### 38.C — `OidcAuth.watch_mtls_provider(provider)`

Mirrors the Phase 37 Rust API:

```python
oidc = (
    await tako.compat.OidcAuth.discover(issuer, audience)
).with_introspection(client_id, secret).with_introspection_self_signed_mtls(
    initial_cert, initial_key,
)

async def fetch():
    return await my_hsm.issue_cert()

provider = tako.compat.MtlsIdentityProvider(fetch)
_watcher = oidc.watch_mtls_provider(provider)
```

Returns a `PyMtlsProviderWatcher` (parallel to the Phase 35
`PyMtlsFsWatcher` — `shutdown()` + `__enter__`/`__exit__`).

### 38.D — `python/tako/compat.py` re-exports

```python
MtlsIdentityProvider = getattr(_native, "MtlsIdentityProvider", None)
MtlsProviderWatcher = getattr(_native, "MtlsProviderWatcher", None)
```

Both added to `__all__`.

### 38.E — Recipe doc

The Phase 37 "Trait-based identity provider" section currently
shows Rust only. Phase 38 adds a Python sub-example.

### 38.F — Smoke test

Mirrors the Phase 35 `test_phase35_oidc_mtls_fs_watcher.py`
shape: facade attribute presence + protocol checks. Skip when
`MtlsIdentityProvider` is `None` (slim wheel).

### 38.G — Version bump 0.38.0 → 0.39.0

### 38.H — PLAN.md

- New row.
- Drop the "Python facade for `MtlsIdentityProvider`" entry
  from the Phase 39 candidates (now shipped). "Auto refresh-on-
  handshake-failure" remains as the only Phase 33 carry-forward.

### 38.I — CHANGELOG `[0.39.0]`

## Critical files

**Modified:**
- [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) — `PyMtlsIdentityProvider` + `PyMtlsProviderWatcher` + `OidcAuth.watch_mtls_provider`.
- [`crates/tako-py/src/lib.rs`](../crates/tako-py/src/lib.rs) — register the new pyclasses.
- [`crates/tako-py/Cargo.toml`](../crates/tako-py/Cargo.toml) — `auth-mtls-identity-provider` feature.
- [`python/tako/compat.py`](../python/tako/compat.py) — re-exports.
- [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) — Python sub-example.
- Version bump + PLAN/CHANGELOG.

**Created:**
- [`tests/python/test_phase38_mtls_provider_python.py`](../tests/python/test_phase38_mtls_provider_python.py).
- [`plans/PLAN_PHASE38.md`](PLAN_PHASE38.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy -p tako-py --features "auth-mtls-identity-provider auth-oidc auth-jwt auth-vault" -- -D warnings`.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
4. `cargo test --workspace --exclude tako-py --all-features`.
5. `maturin develop --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider sigstore sigstore-protobuf redis ws grpc"` — wheel builds at v0.39.0.
6. `pytest -q`.
7. `ruff format --check` + `ruff check`.

## Out of scope

- **Auto refresh-on-handshake-failure.** Still the only Phase
  33 carry-forward remaining. Will sit on top of Phase 35 /
  Phase 37 rotation sources via a future
  `MtlsRefreshHook` trait. Phase 39+ candidate.
- **Synchronous-callable Python providers.** The trait is
  inherently async (matches the Rust trait); operators with
  blocking HSM SDKs wrap in `asyncio.to_thread(...)` inside
  their coroutine.
