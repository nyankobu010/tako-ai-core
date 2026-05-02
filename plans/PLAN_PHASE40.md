# PLAN — Phase 40 (Python facade for `MtlsRefreshHook`)

> **Status: in progress.** Targets v0.41.0. Carry-forward from
> [Phase 39](PLAN_PHASE39.md) — closes the Python facade
> deferred when Phase 39 shipped the Rust-side primitive.

## Context

Phase 39 (v0.40.0) shipped Rust-only:

- `MtlsRefreshHook` Clone-able handle wrapping a one-shot RPC
  channel between the introspection POST retry layer and a
  Phase 35 watcher / Phase 37 provider's background task.
- `OidcAuthResolver::with_mtls_refresh_hook(hook)` builder that
  enables auto-retry on `TakoError::Transport`.
- `MtlsFsWatcher::refresh_hook()` and
  `MtlsProviderWatcher::refresh_hook()` — return the wired hook.

Python-wheel operators using `OidcAuth.watch_mtls_files` (Phase
35.B) or `OidcAuth.watch_mtls_provider` (Phase 38) currently get
no auto-retry. The retry layer runs entirely on the Rust side
once `with_mtls_refresh_hook` is wired — there's no Python
inheritance / async-trait gymnastics involved. Bridging the
hook is mechanical: one new pyclass, two `refresh_hook()`
accessors, one builder method.

## Why now

Phase 39's Rust-side primitive is the load-bearing part; this
phase is the small bookend. Closing it puts Python wheel
operators at parity with Rust on the entire Phase 33 mTLS
rotation surface (both proactive cadences + the reactive
retry layer).

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 40.A | `PyMtlsRefreshHook` pyclass + `OidcAuth.with_mtls_refresh_hook(hook)` | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) |
| 40.B | `MtlsFsWatcher.refresh_hook()` Python method (Phase 35 feature) | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) |
| 40.C | `MtlsProviderWatcher.refresh_hook()` Python method (Phase 37 feature) | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs) |
| 40.D | Pyclass registration | [`crates/tako-py/src/lib.rs`](../crates/tako-py/src/lib.rs) |
| 40.E | Re-export from facade | [`python/tako/compat.py`](../python/tako/compat.py) |
| 40.F | Python smoke test | [`tests/python/test_phase40_mtls_refresh_hook_python.py`](../tests/python/test_phase40_mtls_refresh_hook_python.py) |
| 40.G | Recipe doc — Python sub-example for the Phase 39 retry section | [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) |
| 40.H | Workspace + Python version 0.40.0 → 0.41.0 | various |
| 40.I | PLAN.md row + Phase 41 candidate refresh | [`PLAN.md`](../PLAN.md) |
| 40.J | CHANGELOG.md `[0.41.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 40.A — `PyMtlsRefreshHook` pyclass + builder

```rust
#[cfg(feature = "auth-oidc")]
#[pyclass(name = "MtlsRefreshHook", module = "tako._native", skip_from_py_object)]
#[derive(Clone)]
pub struct PyMtlsRefreshHook {
    inner: tako_compat::MtlsRefreshHook,
}

#[cfg(feature = "auth-oidc")]
#[pymethods]
impl PyMtlsRefreshHook {
    fn __repr__(&self) -> String { "MtlsRefreshHook(...)".into() }
}
```

Operators don't construct this directly; the
`refresh_hook()` accessors on the Phase 35 / Phase 37 watcher
pyclasses return one. (Matches the Rust API.)

`OidcAuth.with_mtls_refresh_hook(hook)` Python method (gated
on `auth-oidc`):

```rust
fn with_mtls_refresh_hook(&self, hook: PyRef<'_, PyMtlsRefreshHook>) -> Self {
    let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
    let next = cloned.with_mtls_refresh_hook(hook.inner.clone());
    Self { inner: Arc::new(next) }
}
```

Returns a NEW `OidcAuth` (immutable builder; matches the Phase
14 / 15 cadence) — same shape as `with_introspection_*`.

### 40.B + 40.C — `refresh_hook()` accessors on watcher pyclasses

`MtlsFsWatcher.refresh_hook()` (gated on `auth-mtls-fs-watch`)
and `MtlsProviderWatcher.refresh_hook()` (gated on
`auth-mtls-identity-provider`) both return a fresh
`PyMtlsRefreshHook`. Implementation:

```rust
fn refresh_hook(&self) -> PyResult<PyMtlsRefreshHook> {
    let inner = self.inner.as_ref()
        .ok_or_else(|| PyValueError::new_err("watcher has been shut down"))?;
    Ok(PyMtlsRefreshHook { inner: inner.refresh_hook() })
}
```

Returns `ValueError` if the watcher has already been
shut down (the inner `Option` is `None` after `shutdown()`).

### 40.D — Pyclass registration in `lib.rs`

`PyMtlsRefreshHook` is registered when `auth-oidc` is on
(matches the gate of its parent `OidcAuth`).

### 40.E — `python/tako/compat.py` re-export

```python
MtlsRefreshHook = getattr(_native, "MtlsRefreshHook", None)
```

Added to `__all__`.

### 40.F — Smoke test

`tests/python/test_phase40_mtls_refresh_hook_python.py`:

1. Facade attribute presence + non-None when `auth-oidc` is on.
2. `OidcAuth.with_mtls_refresh_hook` is callable.
3. `MtlsFsWatcher.refresh_hook()` exists when
   `auth-mtls-fs-watch` is on.
4. `MtlsProviderWatcher.refresh_hook()` exists when
   `auth-mtls-identity-provider` is on.
5. `MtlsRefreshHook` in `__all__` even on slim wheels.

### 40.G — Recipe doc

The Phase 39 "Auto-retry on TLS handshake failure" section
shows a Rust example. Phase 40 adds a Python sub-example
underneath.

### 40.H — Version bump

0.40.0 → 0.41.0 across `Cargo.toml`, `pyproject.toml`,
`python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 40.I — PLAN.md

- New row.
- Drop "Python facade for `MtlsRefreshHook`" from Phase 41
  candidates (now shipped). The only remaining mTLS-related
  carry-forward is the `jsonwebtoken` 10.x migration (a
  fix-it task, not a feature).

### 40.J — CHANGELOG `[0.41.0]`

## Critical files

**Modified:**
- [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs)
- [`crates/tako-py/src/lib.rs`](../crates/tako-py/src/lib.rs)
- [`python/tako/compat.py`](../python/tako/compat.py)
- [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md)
- Standard PLAN/CHANGELOG/version flip.

**Created:**
- [`tests/python/test_phase40_mtls_refresh_hook_python.py`](../tests/python/test_phase40_mtls_refresh_hook_python.py)
- [`plans/PLAN_PHASE40.md`](PLAN_PHASE40.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy -p tako-py --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider" -- -D warnings`.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
4. `cargo test --workspace --exclude tako-py --all-features`.
5. `ruff format --check` + `ruff check`.
6. `pytest -q`.
7. `maturin develop --features "..."` — wheel builds at v0.41.0.

## Out of scope

- **`MtlsRefreshHook.force_refresh()` exposed from Python.**
  Operators rarely need to trigger refreshes manually from
  Python — the hook's job is to be wired once at startup,
  not called per-request. Keeping the surface read-only
  matches the Rust public API (the `force_refresh` method is
  there, but the entire retry path is internal).
- **`jsonwebtoken` 10.x migration.** Still a Phase 41+
  candidate.
