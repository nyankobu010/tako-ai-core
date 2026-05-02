# PLAN — Phase 39 (Auto refresh-on-handshake-failure for OIDC mTLS)

> **Status: in progress.** Targets v0.40.0. Closes the last
> Phase 33 mTLS-rotation carry-forward (strategy 2-of-3 was
> deferred Phase 33; strategy 3 shipped in Phase 35; strategy 1
> shipped in Phase 37). After Phase 39 the Phase 33 carry-
> forward list is empty.

## Context

Three rotation strategies were sketched in
[Phase 33](PLAN_PHASE33.md):

| # | Strategy | Status |
|---|----------|--------|
| 1 | Trait-based identity provider | Phase 37 (Rust) + Phase 38 (Python) |
| 2 | **Auto refresh-on-handshake-failure** | **This phase** |
| 3 | Filesystem-watcher integration | Phase 35 |

Strategies 1 + 3 cover **proactive** rotation — the watcher /
provider refreshes before the cert expires. Strategy 2 covers
the **reactive** path: when the introspection POST gets a
transport error mid-flight (e.g. cert was revoked, the issuer
rotated its trust store, an out-of-band rotation desynchronised
us), auto-trigger a refresh and retry once.

Without Phase 39, transport errors during introspection bubble
up directly to the request handler. The next request makes the
same broken POST. The Phase 35 / Phase 37 watcher would
eventually refresh on its normal schedule, but in the gap
**every introspection attempt fails**.

Phase 39 closes that gap. The retry layer:

1. Detects a transport error from the introspection POST.
2. Triggers an out-of-band refresh via a configured
   [`MtlsRefreshHook`].
3. Waits for the refresh to land (with a timeout).
4. Retries the POST exactly once. If it also fails, propagates.

Cycle-detection is built-in: at most one retry per
introspection call. A persistent issuer outage cannot loop
indefinitely.

## Why now

After Phase 38, both the Phase 35 watcher and the Phase 37
trait-based provider are stable surfaces with the same shape
(both spawn a tokio task that does an out-of-band reload).
Adding a force-refresh hook on top is mechanical; both
watchers gain a single new `select!` arm.

After Phase 39, the Phase 33 backlog is fully retired and the
mTLS rotation surface is feature-complete for the
foreseeable future. Future phases can move on to the broader
backlog (`TakoError::Provider` short-circuit refinement,
Vertex File API, eval graders, etc.).

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 39.A | `MtlsRefreshHook` value type + completion-signalling primitive | new module [`crates/tako-compat/src/auth/oidc_mtls_hook.rs`](../crates/tako-compat/src/auth/oidc_mtls_hook.rs) |
| 39.B | `OidcAuthResolver::with_mtls_refresh_hook(hook)` builder + retrieval accessor | [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs) |
| 39.C | Phase 35 `MtlsFsWatcher` exposes a hook + adds the trigger arm to its `select!` loop | [`crates/tako-compat/src/auth/oidc_mtls_watcher.rs`](../crates/tako-compat/src/auth/oidc_mtls_watcher.rs) |
| 39.D | Phase 37 `MtlsProviderWatcher` exposes a hook + adds the trigger arm | [`crates/tako-compat/src/auth/oidc_mtls_provider.rs`](../crates/tako-compat/src/auth/oidc_mtls_provider.rs) |
| 39.E | Introspection POST retry layer in `OidcAuthResolver::introspect` | [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs) |
| 39.F | Rust tests covering retry semantics + cycle-detection | inline `#[cfg(test)]` in `oidc.rs` |
| 39.G | Recipe doc — when reactive refresh fires + the hook concept | [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) |
| 39.H | Workspace + Python version 0.39.0 → 0.40.0 | various |
| 39.I | PLAN.md row + Phase 40 candidate refresh | [`PLAN.md`](../PLAN.md) |
| 39.J | CHANGELOG.md `[0.40.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 39.A — `MtlsRefreshHook`

New public type in a new module `oidc_mtls_hook.rs` (gated on
`oidc` feature so it's available alongside the introspection
retry path even when neither Phase 35 nor Phase 37 features
are on):

```rust
/// Phase 39 — handle that triggers an out-of-band mTLS
/// identity refresh from a Phase 35 filesystem watcher or a
/// Phase 37 trait-based provider.
///
/// Operators rarely construct this directly. Both
/// [`MtlsFsWatcher::refresh_hook`] and
/// [`MtlsProviderWatcher::refresh_hook`] return a
/// fully-wired hook that drives the corresponding watcher's
/// background task. Pass the hook to
/// [`OidcAuthResolver::with_mtls_refresh_hook`] to enable
/// auto-retry on TLS-handshake failure.
#[derive(Clone)]
pub struct MtlsRefreshHook {
    inner: Arc<MtlsRefreshHookInner>,
}

struct MtlsRefreshHookInner {
    trigger_tx: mpsc::Sender<oneshot::Sender<Result<(), TakoError>>>,
}

impl MtlsRefreshHook {
    /// Trigger an out-of-band reload from the wired refresh
    /// source. Awaits the source's response (cert read +
    /// reqwest::Client build); returns the source's error if
    /// the reload itself fails.
    ///
    /// Capped at 2 seconds; longer reloads time out and the
    /// caller's retry will see the previously installed
    /// Client (preserved per Phase 33 semantics).
    pub async fn force_refresh(&self) -> Result<(), TakoError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.inner.trigger_tx.send(resp_tx).await
            .map_err(|_| TakoError::Invalid(
                "oidc.mtls_refresh_hook: refresh source dropped".into(),
            ))?;
        match tokio::time::timeout(REFRESH_TIMEOUT, resp_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(TakoError::Invalid(
                "oidc.mtls_refresh_hook: refresh source dropped reply".into(),
            )),
            Err(_) => Err(TakoError::Invalid(format!(
                "oidc.mtls_refresh_hook: refresh timed out after {:?}",
                REFRESH_TIMEOUT,
            ))),
        }
    }
}
```

The receiving end (also exposed for the watcher integrations):

```rust
pub(crate) struct MtlsRefreshTrigger {
    pub(crate) trigger_rx: mpsc::Receiver<oneshot::Sender<Result<(), TakoError>>>,
}

pub(crate) fn refresh_channel() -> (MtlsRefreshHook, MtlsRefreshTrigger) {
    let (tx, rx) = mpsc::channel(1);
    (
        MtlsRefreshHook { inner: Arc::new(MtlsRefreshHookInner { trigger_tx: tx }) },
        MtlsRefreshTrigger { trigger_rx: rx },
    )
}
```

### 39.B — Resolver builder

```rust
impl OidcAuthResolver {
    /// Phase 39 — wire a refresh hook from a Phase 35
    /// `MtlsFsWatcher` or Phase 37 `MtlsProviderWatcher` so
    /// the introspection POST can trigger an out-of-band
    /// reload on a transport-error retry.
    pub fn with_mtls_refresh_hook(mut self, hook: MtlsRefreshHook) -> Self {
        self.mtls_refresh_hook = Some(hook);
        self
    }
}
```

`OidcAuthResolver` gains a private `mtls_refresh_hook:
Option<MtlsRefreshHook>` field. When `None`, the introspection
POST behaves exactly as before (Phase 24/25/33/34 byte-for-
byte).

### 39.C — Phase 35 `MtlsFsWatcher` integration

Constructor change: `watch_mtls_files` returns the watcher
and ALSO accepts a `MtlsRefreshTrigger` it consumes into the
background task's select loop. To preserve back-compat, the
existing API stays unchanged and a NEW method
`refresh_hook()` returns a `MtlsRefreshHook` that the
operator pairs with `with_mtls_refresh_hook` separately:

```rust
impl OidcAuthResolver {
    pub fn watch_mtls_files(
        self: Arc<Self>,
        cert_path: PathBuf,
        key_path: PathBuf,
    ) -> Result<MtlsFsWatcher, TakoError>;
    // unchanged
}

impl MtlsFsWatcher {
    /// Phase 39 — return a `MtlsRefreshHook` wired to this
    /// watcher's background task. Pair with
    /// `OidcAuthResolver::with_mtls_refresh_hook` to enable
    /// auto-retry of failed introspection POSTs.
    pub fn refresh_hook(&self) -> MtlsRefreshHook;
}
```

Internally, `watch_mtls_files` creates the channel pair at
construction, stores the `MtlsRefreshHook` clone on the
watcher, and feeds the `MtlsRefreshTrigger` to the task. The
task's select loop adds:

```rust
Some(resp_tx) = trigger.trigger_rx.recv() => {
    let result = perform_reload(&resolver, &cert_path, &key_path).await;
    let _ = resp_tx.send(result);
}
```

### 39.D — Phase 37 `MtlsProviderWatcher` integration

Same pattern. `MtlsProviderWatcher::refresh_hook()` returns
the wired hook; the watcher's loop adds the corresponding
trigger arm that calls `provider.fetch().await` then
`resolver.reload_mtls_identity(...)` and signals back.

### 39.E — Introspection POST retry layer

Refactor the existing send-and-classify in `introspect()`
into an internal helper `introspect_send_once` that returns
`Result<reqwest::Response, TakoError>`. The `introspect()`
caller wraps:

```rust
let resp = match self.introspect_send_once(/* ... */).await {
    Ok(r) => r,
    Err(TakoError::Transport(_)) if self.mtls_refresh_hook.is_some()
        && cfg.mtls_client.is_some() => {
        // Phase 39 — TLS / transport error on an mTLS-enabled
        // introspection POST. Trigger out-of-band refresh, then
        // retry exactly once.
        if let Some(hook) = &self.mtls_refresh_hook {
            if let Err(refresh_err) = hook.force_refresh().await {
                tracing::warn!(
                    error = %refresh_err,
                    "oidc.introspect: force_refresh failed; retrying with current client"
                );
            }
        }
        self.introspect_send_once(/* ... */).await?
    }
    Err(e) => return Err(e),
};
```

Cycle detection is implicit in the structure: the retry path
calls `introspect_send_once` (no further retry). At most one
retry per `introspect()` call.

The retry only fires when:
1. The error is a `TakoError::Transport`.
2. A refresh hook is configured.
3. mTLS is configured (`cfg.mtls_client.is_some()`).

Non-mTLS auth methods (Basic, Post, JWT) skip the retry —
they have no per-request identity to refresh.

### 39.F — Tests

Inline `#[cfg(test)] mod tests` in `oidc.rs` (joining the
existing test module). New tests:

1. `introspect_retry_fires_on_transport_error_with_hook` — a
   counting hook + a wiremock that fails the first POST and
   succeeds the second; assert hook was called once + retry
   succeeded.
2. `introspect_retry_skipped_without_hook` — wiremock fails
   first POST; no hook configured; assert single POST attempt
   + transport error propagates.
3. `introspect_retry_skipped_for_non_mtls_method` — same
   setup but `auth_method = ClientSecretBasic`; assert single
   attempt + no hook call.
4. `introspect_retry_propagates_on_second_failure` — wiremock
   fails both POSTs; assert hook was called once + final
   error is transport.
5. `introspect_retry_continues_when_force_refresh_fails` — a
   counting hook that returns `Err`; assert second POST still
   fires and the original transport error propagates from the
   second attempt.
6. `force_refresh_times_out_when_source_dropped` — construct
   a `MtlsRefreshHook` whose paired trigger is dropped;
   `force_refresh().await` returns `TakoError::Invalid`
   carrying "refresh source dropped".
7. `force_refresh_times_out_after_2s` — paired trigger that
   never replies; `force_refresh().await` returns
   `TakoError::Invalid` carrying "timed out".

Plus integration tests for the watcher hook wiring (under
their respective feature gates):

8. `mtls_fs_watcher_force_refresh_triggers_reload` (Phase 35
   feature) — call `watcher.refresh_hook().force_refresh()`
   directly; assert `MtlsClient` Arc swaps.
9. `mtls_provider_watcher_force_refresh_triggers_reload`
   (Phase 37 feature) — same shape.

### 39.G — Recipe doc

Add a "Auto-retry on TLS handshake failure (Phase 39)"
section to [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md).
Show the wiring:

```python
watcher = oidc.watch_mtls_files(cert_path, key_path)
oidc = oidc.with_mtls_refresh_hook(watcher.refresh_hook())
```

(Or the equivalent for the provider.) Document the
trigger semantics: only fires on `TakoError::Transport`,
only when mTLS is configured, exactly one retry per
introspection call.

### 39.H — Version bump

`Cargo.toml` workspace + every `path = "..."` workspace dep
version. `pyproject.toml`. `python/tako/__init__.py`.
`tests/python/test_smoke.py`'s pinned assertion.

### 39.I — PLAN.md

- New row.
- Replace "Phase 39 candidates" with "Phase 40 candidates".
  Drop the `Automatic refresh-on-handshake-failure` entry
  (now shipped). After Phase 39 the Phase 33 carry-forward
  list is empty; a new top-level note states this.

### 39.J — CHANGELOG `[0.40.0]`

## Critical files

**Modified:**
- [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs) — `with_mtls_refresh_hook` + `mtls_refresh_hook` field + retry layer + tests.
- [`crates/tako-compat/src/auth/oidc_mtls_watcher.rs`](../crates/tako-compat/src/auth/oidc_mtls_watcher.rs) — `refresh_hook()` + select-arm.
- [`crates/tako-compat/src/auth/oidc_mtls_provider.rs`](../crates/tako-compat/src/auth/oidc_mtls_provider.rs) — `refresh_hook()` + select-arm.
- [`crates/tako-compat/src/auth/mod.rs`](../crates/tako-compat/src/auth/mod.rs) — re-export `MtlsRefreshHook`.
- [`crates/tako-compat/src/lib.rs`](../crates/tako-compat/src/lib.rs) — re-export.
- [`docs/recipes/mtls_rotation.md`](../docs/recipes/mtls_rotation.md) — new section.
- Standard PLAN/CHANGELOG/version flip.

**Created:**
- [`crates/tako-compat/src/auth/oidc_mtls_hook.rs`](../crates/tako-compat/src/auth/oidc_mtls_hook.rs) — `MtlsRefreshHook` + paired trigger primitive.
- [`plans/PLAN_PHASE39.md`](PLAN_PHASE39.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy -p tako-compat --all-features -- -D warnings`.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
4. `cargo test -p tako-compat --all-features`.
5. `cargo test --workspace --exclude tako-py --all-features`.
6. `ruff format --check` + `ruff check`.
7. `pytest -q`.
8. `maturin develop --features "..."` builds at v0.40.0.

## Out of scope

- **Python facade for `MtlsRefreshHook` / `with_mtls_refresh_hook`.**
  The Phase 35 `OidcAuth.watch_mtls_files` and Phase 38
  `OidcAuth.watch_mtls_provider` are the operator-facing
  Python entry points; Phase 39 wires the Rust-side
  primitive. A Python facade would be a small follow-on:
  expose `watcher.refresh_hook()` and
  `oidc.with_mtls_refresh_hook(...)` through PyO3. Deferred
  to Phase 40+ because the Rust-side retry layer is the
  load-bearing part — Python wheel operators using the
  facade get auto-retry transparently once they wire the
  hook on the Python side.
- **Per-call retry policy knobs.** Phase 39 hardcodes "retry
  once on Transport error". Operators wanting different
  policies (e.g. retry twice with backoff, retry on Provider
  errors) should layer their own retry above the resolver.
- **Trace span on the retry.** The retry is logged via
  `tracing::warn!` but not given its own OTel span. Future
  enhancement.
- **Coalescing concurrent retries.** N concurrent
  introspection POSTs that all hit a transport error all
  trigger force_refresh. The mpsc channel has bounded
  capacity (1) so the second-onward triggers wait briefly
  for the first to drain. Acceptable: it's a transient
  failure path.
