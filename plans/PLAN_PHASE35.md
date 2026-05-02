# PLAN — Phase 35 (OIDC mTLS filesystem-watcher auto-reload)

> **Status: in progress.** Targets v0.36.0. Carry-forward from
> [Phase 33](PLAN_PHASE33.md) — the third of the three deferred
> rotation strategies (1: explicit-reload — shipped Phase 33;
> 2: trait-based identity provider — deferred Phase 36+;
> 3: filesystem-watcher integration — **this phase**).

## Context

Phase 33 (v0.34.0) shipped the explicit-reload primitive
[`OidcAuthResolver::reload_mtls_identity`](../crates/tako-compat/src/auth/oidc.rs)
with an atomic-swap `MtlsClient` newtype. The recipe doc
[`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md)
suggests operators wire that primitive to a hand-rolled polling
loop:

```python
async def watch_certs(oidc):
    while True:
        await asyncio.sleep(60)
        cert = open("/var/run/secrets/oidc-mtls.crt").read()
        key = open("/var/run/secrets/oidc-mtls.key").read()
        oidc.reload_mtls_identity(cert, key)
```

This works but has three rough edges every operator hits:

1. **Polling latency.** A 60-second poll means up to 60s of TLS
   failures after a cert-manager rotation before pickup. Tightening
   the poll burns cycles re-reading unchanged files.
2. **Atomic-rename detection.** cert-manager / kubernetes secret
   mounts use atomic rename (write to `..data` then symlink swap) —
   inotify's `IN_MOVED_TO` on the symlink target fires once on
   rotation; polling races against partial reads if it happens
   mid-swap.
3. **Boilerplate.** Every operator writes the same loop, often
   missing the "preserve old client on PEM-parse error" semantics
   that Phase 33 already guarantees inside `reload_mtls_identity`.

Phase 35 ships a one-call helper that wraps the `notify` crate
behind a feature flag. Operators call
`OidcAuthResolver::watch_mtls_files(cert_path, key_path)` once at
startup, hold the returned `MtlsFsWatcher` handle for the lifetime
of the resolver, and rotation Just Works.

## Why now (and why not the other two Phase 33 carry-forwards)

The three deferred strategies from Phase 33 are:

| # | Carry-forward | Phase 35 status | Why |
|---|---------------|-----------------|-----|
| 1 | Trait-based `MtlsIdentityProvider` | Deferred | Needs cert-parsing on the tako side (`x509-parser` dep or hand-rolled DER walk) to know when to call back. Larger surface; no operator ask yet. |
| 2 | Automatic refresh-on-handshake-failure | Deferred | Needs retry logic + cycle-detection inside introspection POST. Should sit on top of (1) or filesystem-watcher, not standalone. |
| 3 | **Filesystem-watcher integration** | **This phase** | Smallest concrete operator value; matches how cert-manager / Vault PKI / SPIRE all already publish certs (files on disk). Builds straight on the Phase 33 primitive. |

After Phase 35 lands, (2) becomes a thin layer on top of either
(1) or (3); (1) remains a separate effort that adds proactive
expiry-driven rotation. The full triad is no longer a single
phase.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 35.A | `MtlsFsWatcher` Rust core + `notify` dep + `mtls-fs-watch` cargo feature | [`crates/tako-compat/src/auth/oidc_mtls_watcher.rs`](../crates/tako-compat/src/auth/oidc_mtls_watcher.rs) (new), [`crates/tako-compat/src/auth/mod.rs`](../crates/tako-compat/src/auth/mod.rs), [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs), [`crates/tako-compat/Cargo.toml`](../crates/tako-compat/Cargo.toml), [`Cargo.toml`](../Cargo.toml) |
| 35.B | Python facade `OidcAuth.watch_mtls_files(cert_path, key_path)` + `PyMtlsFsWatcher` handle | [`crates/tako-py/src/compat.rs`](../crates/tako-py/src/compat.rs), [`python/tako/_native.pyi`](../python/tako/_native.pyi), [`python/tako/compat.py`](../python/tako/compat.py) |
| 35.C | Recipe doc — replace polling-loop pattern with the new helper | [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) |
| 35.D | Workspace + Python version 0.35.0 → 0.36.0 | [`Cargo.toml`](../Cargo.toml), [`pyproject.toml`](../pyproject.toml) |
| 35.E | PLAN.md row + Phase 36 candidate-list refresh | [`PLAN.md`](../PLAN.md) |
| 35.F | CHANGELOG.md `[0.36.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 35.A — Rust core: `MtlsFsWatcher`

A new module `oidc_mtls_watcher.rs` (~150 lines) sibling to
`oidc.rs`. Gated behind the new `mtls-fs-watch` cargo feature; the
default `tako-compat` build sees no new transitive deps. The
feature implies `oidc` (the watcher only makes sense if
introspection mTLS is wired up).

Public surface on `OidcAuthResolver` (in `oidc.rs`, gated on the
feature):

```rust
#[cfg(feature = "mtls-fs-watch")]
pub fn watch_mtls_files(
    self: Arc<Self>,
    cert_path: PathBuf,
    key_path: PathBuf,
) -> Result<MtlsFsWatcher, TakoError>;
```

Returns a `MtlsFsWatcher` handle. The handle:

- **Owns** the `notify::RecommendedWatcher` and a `tokio::task::JoinHandle`.
- **Holds** an `Arc<OidcAuthResolver>` so the watcher's reload
  closure can call `reload_mtls_identity` without an Arc-cycle
  (resolver does not own the watcher; watcher owns the resolver
  Arc).
- **Drops cleanly:** the `Drop` impl signals `tokio::sync::Notify`
  to wake the background task, which drops the `notify::Watcher`
  and exits. No leaked threads.
- **Exposes** `shutdown(self)` for explicit, awaitable teardown
  (the bare `Drop` is fire-and-forget, which is fine in production
  but awkward in tests that want to assert no events fire after
  shutdown). `shutdown` calls `JoinHandle::abort` and returns once
  the abort completes.

Watcher behaviour:

- Watches the **parent directory** of each cert/key path, not the
  files themselves. This is mandatory for cert-manager /
  kubernetes-secret atomic-rename rotation: the inode of the
  inner file changes, so a watch on the path itself goes stale.
  Watching the parent dir + filtering events by filename matches
  the inotify(7) recommendation.
- **Coalesces bursts** with a 500 ms debounce. Cert-manager
  writes both cert and key in quick succession; a single reload
  per debounce window is the right cadence.
- **Reload errors do not kill the watcher.** A `tracing::warn!`
  records the failure; the next change event triggers another
  attempt. Operators watching their `tako.compat.mtls.reload.*`
  spans / log lines see every failure but the server keeps
  serving on the old (still-valid) client.
- **Initial reload is implicit.** The resolver was constructed
  with `with_introspection_mtls(initial_cert, initial_key)` before
  `watch_mtls_files` was called; the watcher's job is rotation,
  not bootstrap. We do *not* trigger a synthetic reload at
  startup — that would defeat the "previously installed client
  preserved on parse error" guarantee in the boot path.

Error surfaces (all `TakoError::Invalid` with operator-friendly
messages):

| Failure | Behaviour |
|---------|-----------|
| `cert_path` or `key_path` parent dir does not exist | Construction fails. |
| `notify::Watcher` setup fails (kernel limit, permission) | Construction fails; underlying error formatted into the message. |
| Reload fails post-debounce (PEM parse, Client build) | `warn!`; previous client preserved per Phase 33 semantics. |
| Resolver was never configured with mTLS | Construction fails (mirrors the existing `reload_mtls_identity` "no mTLS configured" check). |

#### A.1 — Module skeleton

```rust
//! Phase 35 — filesystem-watcher integration for OIDC mTLS
//! cert/key auto-reload.
//!
//! Wraps the `notify` crate behind the `mtls-fs-watch` cargo
//! feature so operators using cert-manager / Vault PKI / SPIRE
//! / kubernetes-secret-mount rotation can call
//! [`OidcAuthResolver::watch_mtls_files`] once at startup
//! instead of hand-rolling a polling loop on top of
//! [`OidcAuthResolver::reload_mtls_identity`].

use notify::{
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::ModifyKind,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use crate::auth::oidc::OidcAuthResolver;
use tako_core::TakoError;

const DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub struct MtlsFsWatcher {
    _watcher: RecommendedWatcher,  // dropping ends fs notifications
    shutdown: Arc<Notify>,
    join: Option<JoinHandle<()>>,
}

impl Drop for MtlsFsWatcher {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        if let Some(h) = self.join.take() {
            h.abort();
        }
    }
}

impl MtlsFsWatcher {
    pub async fn shutdown(mut self) { /* explicit await variant */ }
}
```

#### A.2 — Construction logic

```rust
impl OidcAuthResolver {
    pub fn watch_mtls_files(
        self: Arc<Self>,
        cert_path: PathBuf,
        key_path: PathBuf,
    ) -> Result<MtlsFsWatcher, TakoError> {
        // 1. Verify mTLS is configured (mirrors reload_mtls_identity).
        // 2. Verify both parent dirs exist.
        // 3. Create RecommendedWatcher with a tokio mpsc-bridged callback.
        // 4. Spawn background task that consumes mpsc events, debounces
        //    by 500ms, then reads + reloads.
        // 5. Return MtlsFsWatcher holding watcher + JoinHandle + shutdown Notify.
    }
}
```

#### A.3 — Tests

`crates/tako-compat/tests/mtls_fs_watcher.rs` — 4 tests, all gated
on the `mtls-fs-watch` + `oidc` features:

1. `cert_change_triggers_reload` — write initial files via
   `tempfile`, construct resolver + watcher, atomically rewrite
   cert via `std::fs::rename` of a temp file, sleep 1s, assert a
   reload was observed (track via a counter in a custom
   `MtlsClient` swap path, OR by snapshotting `MtlsClient::current`
   pointer-equality before/after). Use the latter — keeps the test
   black-box.
2. `key_change_triggers_reload` — same as above for the key path.
3. `parse_failure_preserves_client` — write bad PEM; assert
   `MtlsClient::current` pointer is unchanged after the debounce
   window. (Logs go to a `tracing` test subscriber; we just check
   the client didn't swap.)
4. `drop_stops_watcher` — drop the `MtlsFsWatcher`; rewrite the
   cert; assert no reload is observed for 1s.

All four tests use `tokio::time::sleep` with a generous 1s margin
above the 500ms debounce — flake-prone otherwise.

#### A.4 — Cargo manifest

`Cargo.toml` (workspace):
```toml
notify = "8"
```

`crates/tako-compat/Cargo.toml`:
```toml
[dependencies]
notify = { workspace = true, optional = true }

[features]
# Phase 35 — opt-in filesystem-watcher integration for OIDC
# mTLS cert/key auto-reload. Requires `oidc` (the watcher only
# makes sense if mTLS introspection is wired up).
mtls-fs-watch = ["oidc", "dep:notify"]
```

The default `tako-compat` build (no features) keeps its current
dep tree. Operators opting into mTLS rotation flip on
`tako-compat/mtls-fs-watch` and pay for the `notify` dep then.

### 35.B — Python facade

#### B.1 — `tako-py` PyO3 binding

In [`crates/tako-py/src/compat.rs`](../crates/tako-py/src/compat.rs):

```rust
#[cfg(feature = "mtls-fs-watch")]
#[pymethods]
impl PyOidcAuth {
    fn watch_mtls_files(
        slf: Py<Self>,
        py: Python<'_>,
        cert_path: PathBuf,
        key_path: PathBuf,
    ) -> PyResult<PyMtlsFsWatcher> {
        // 1. Borrow the inner Arc<OidcAuthResolver>.
        // 2. Call resolver.watch_mtls_files(cert_path, key_path).
        // 3. Wrap returned MtlsFsWatcher in PyMtlsFsWatcher.
    }
}

#[pyclass(name = "MtlsFsWatcher", module = "tako._native")]
pub struct PyMtlsFsWatcher {
    inner: Option<MtlsFsWatcher>,
}

#[pymethods]
impl PyMtlsFsWatcher {
    fn shutdown(&mut self, py: Python<'_>) -> PyResult<()> {
        if let Some(w) = self.inner.take() {
            py.detach(|| {
                runtime().block_on(async { w.shutdown().await });
            });
        }
        Ok(())
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> { slf }
    fn __exit__(&mut self, py: Python<'_>, _t: PyObject, _v: PyObject, _tb: PyObject) -> PyResult<bool> {
        self.shutdown(py)?;
        Ok(false)
    }
}
```

The Python pyclass implements `__enter__` / `__exit__` so the
ergonomic Python form is `with oidc.watch_mtls_files(...) as w:`.
For top-level / lifetime-of-process use, operators bind the
handle to a module-scope variable and rely on process exit
(documented in 35.C).

`tako-py` exposes a matching `mtls-fs-watch` cargo feature that
forwards to `tako-compat/mtls-fs-watch`. The wheel built by
`maturin develop --release` defaults to off; operators opt in
via `maturin develop --release --features tako-compat-mtls-fs-watch`
(matching the existing `tako-compat-vault` / `tako-compat-oidc`
feature-forwarding pattern in `tako-py/Cargo.toml`).

#### B.2 — `_native.pyi` type stubs

```python
class MtlsFsWatcher:
    def shutdown(self) -> None: ...
    def __enter__(self) -> "MtlsFsWatcher": ...
    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool: ...

class OidcAuth:
    # ... existing methods ...
    def watch_mtls_files(self, cert_path: str, key_path: str) -> MtlsFsWatcher: ...
```

#### B.3 — `python/tako/compat.py` re-export

`MtlsFsWatcher` is re-exported from `tako.compat` alongside
`OidcAuth`. Stub-only — no Python-level logic.

### 35.C — Recipe doc

[`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md)
gets a new "Filesystem-watcher (recommended)" section above the
existing polling-loop example. The polling-loop pattern is kept
as a fallback for operators on platforms where `notify` cannot
be used (very-restricted containers, FUSE filesystems with no
inotify support, or non-default `tako-compat` builds).

New section:

```markdown
## Filesystem watcher (recommended)

If you build with the `mtls-fs-watch` feature on (Python wheel:
`maturin develop --features tako-compat-mtls-fs-watch`; Rust:
`tako-compat = { ..., features = ["mtls-fs-watch"] }`), the
helper wires `notify` to `reload_mtls_identity` for you.

```python
import tako.compat

oidc = (
    tako.compat.OidcAuth(issuer="...", audience="...")
    .with_introspection(client_id="my-api", client_secret="")
    .with_introspection_mtls(
        cert_pem=open("/var/run/secrets/oidc-mtls.crt").read(),
        key_pem=open("/var/run/secrets/oidc-mtls.key").read(),
    )
)

watcher = oidc.watch_mtls_files(
    cert_path="/var/run/secrets/oidc-mtls.crt",
    key_path="/var/run/secrets/oidc-mtls.key",
)

# Hold `watcher` for the lifetime of the resolver. Drop / shutdown
# stops the background task cleanly.
```
```

### 35.D — Version bump

`Cargo.toml` workspace + every `path = "..."` dep version string.
`pyproject.toml`. `_native.pyi` is generated; only the version
constant if any (none — file-level type stubs only).

### 35.E — PLAN.md update

- New row: `| 35 — OIDC mTLS filesystem-watcher auto-reload | v0.36.0 | done (date) | plans/PLAN_PHASE35.md | ## [0.36.0] |`
- Replace the "Phase 35 candidates" section with "Phase 36
  candidates" — same list minus filesystem-watcher (now shipped),
  plus a note that the (1) trait-based provider and (2)
  refresh-on-handshake-failure carry-forwards still stand.
- Roadmap stays unchanged otherwise.

### 35.F — CHANGELOG entry

```markdown
## [0.36.0] - 2026-05-02

### Added
- **`tako-compat`: OIDC mTLS filesystem-watcher auto-reload** — new
  optional cargo feature `mtls-fs-watch` (Python wheel feature
  `tako-compat-mtls-fs-watch`) ships an `OidcAuthResolver::watch_mtls_files(cert_path, key_path)` helper that wraps the `notify` crate to
  auto-call `reload_mtls_identity` whenever the watched cert or key
  files change on disk. Behaviour: watches the *parent directories*
  (atomic-rename safe per cert-manager / kubernetes-secret-mount
  conventions), 500ms debounce, reload failures logged at `warn!`
  without killing the watcher. Returns an `MtlsFsWatcher` handle whose
  `Drop` impl shuts the background task down cleanly. Python facade:
  `oidc.watch_mtls_files(...)` returns a context-manager-friendly
  `MtlsFsWatcher`. Default `tako-compat` build is unchanged — feature
  is opt-in.
```

## Critical files

**Modified:**
- [`Cargo.toml`](../Cargo.toml) — workspace `notify` dep + version
  bump.
- [`crates/tako-compat/Cargo.toml`](../crates/tako-compat/Cargo.toml) —
  optional `notify` dep + `mtls-fs-watch` feature.
- [`crates/tako-compat/src/auth/mod.rs`](../crates/tako-compat/src/auth/mod.rs) —
  feature-gated `pub mod oidc_mtls_watcher` + re-export of
  `MtlsFsWatcher`.
- [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs) —
  new `watch_mtls_files` method on `OidcAuthResolver` (feature-
  gated).
- [`crates/tako-py/Cargo.toml`](../crates/tako-py/Cargo.toml) —
  forward feature.
- [`crates/tako-py/src/compat.rs`](../crates/tako-py/src/compat.rs) —
  Python binding.
- [`python/tako/_native.pyi`](../python/tako/_native.pyi) — stubs.
- [`python/tako/compat.py`](../python/tako/compat.py) — re-export.
- [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) —
  new section.
- [`pyproject.toml`](../pyproject.toml) — version bump.
- [`PLAN.md`](../PLAN.md) — phase index + Phase 36 candidates.
- [`CHANGELOG.md`](../CHANGELOG.md) — `[0.36.0]` entry.

**Created:**
- [`crates/tako-compat/src/auth/oidc_mtls_watcher.rs`](../crates/tako-compat/src/auth/oidc_mtls_watcher.rs).
- [`crates/tako-compat/tests/mtls_fs_watcher.rs`](../crates/tako-compat/tests/mtls_fs_watcher.rs).
- [`plans/PLAN_PHASE35.md`](PLAN_PHASE35.md) (this file).

## Verification

1. `cargo fmt --all -- --check` passes.
2. `cargo clippy -p tako-compat --all-features -- -D warnings` passes.
3. `cargo test -p tako-compat --all-features` passes (incl. new
   `mtls_fs_watcher.rs` integration tests).
4. `cargo clippy --workspace --all-features -- -D warnings` passes
   (excluding `tako-py` if no Python lib in dev env; CI handles
   that).
5. `cargo test --workspace --all-features` passes (same exclusion).
6. `maturin develop --release --features tako-compat-mtls-fs-watch`
   builds and `pytest tests/python/test_compat_mtls_fs_watch.py`
   passes (skipped gracefully if the feature wasn't built in).
7. Default `pip install` smoke (no feature flag): existing tests
   continue to pass.

## Out of scope

- **Trait-based `MtlsIdentityProvider`** — Phase 33 carry-forward,
  separate phase.
- **Automatic refresh-on-handshake-failure** — Phase 33 carry-forward,
  separate phase.
- **Watching arbitrary numbers of cert/key pairs.** Each
  `watch_mtls_files` call attaches one watcher to one resolver.
  Operators with multiple OIDC providers wire one watcher each.
- **Per-event metrics.** Reloads emit a `tracing::info!` /
  `tracing::warn!` line; full `gen_ai.*` / `tako.*` semconv span
  surfaces would land in a later phase if operators ask.
- **Wildcard / glob path patterns.** `watch_mtls_files` takes two
  exact `PathBuf`s. cert-manager / Vault PKI / SPIRE all publish
  to known paths; a glob layer is YAGNI.
