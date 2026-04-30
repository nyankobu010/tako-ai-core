# PLAN — Phase 13 (Multi-replica `StateStore` + streaming-aware Verifier in Trinity)

## Context

Phase 12 (v0.13.0, 2026-04-30) shipped MCP Streamable HTTP SSE
notifications and the `tako.providers.HttpGeneric` Python facade. That
left five Phase 13 candidates pre-listed in
[PLAN.md](/Users/kwc/tako-ai-core/PLAN.md). Phase 13 lands two
strictly-additive items that build on patterns already established in
earlier phases — no new architectural decisions:

- **A.** `StateStore` trait + `RedisStateStore` impl. Phase 10.A's
  [`JsonStateStore`](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state.rs)
  is single-process; multi-replica deployments where multiple workers
  share the same Rekor freshness anchor need shared persistence.
  Mirrors Phase 4's
  [`RedisBudgetBackend`](/Users/kwc/tako-ai-core/crates/tako-runtime/src/budget_redis.rs)
  pattern (gated behind a `redis` cargo feature). Adds a new
  `tako-governance/redis` feature and a parallel `RedisStateStore`
  Python facade reachable via the `tako-py` umbrella `redis` feature.
- **B.** Streaming-aware `Verifier` in Trinity. Phase 10.C emits
  `OrchEvent::VerifierScore` only at synthesis-complete boundaries.
  Operators want per-delta scoring for cheap heuristic verifiers
  (regex pass-rate, length, etc.) so they can short-circuit doomed
  rollouts early. Mirrors Phase 9.A's
  [`ConfidenceGuard::evaluate_streaming`](/Users/kwc/tako-ai-core/crates/tako-core/src/traits/confidence.rs#L54-L60)
  default-impl method and the `LlmJudgeGuard::with_streaming_min_chars`
  cost-control surface. **Trinity only this phase** — Conductor's
  `dispatch_workers().await` returns flat `Vec<WorkerResult>` with no
  intra-worker delta exposure today; refactor deferred (carry-forward
  in [PLAN.md](/Users/kwc/tako-ai-core/PLAN.md) backlog).

**Theme:** *Multi-replica enablement + per-delta verifier scoring.*

**Target tag:** v0.14.0.

## A. `StateStore` trait + `RedisStateStore`

### What ships

#### A.1 — `StateStore` trait extraction

[crates/tako-governance/src/sigstore_state.rs](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state.rs)
gains a public `StateStore` trait (above the existing `JsonStateStore`
struct). Two required methods, two default-impl convenience methods:

```rust
#[async_trait::async_trait]
pub trait StateStore: Send + Sync + std::fmt::Debug + 'static {
    async fn load(&self) -> Result<u64, TakoError>;
    async fn save(&self, n: u64) -> Result<(), TakoError>;

    async fn seed(&self, v: KeylessVerifier) -> Result<KeylessVerifier, TakoError> {
        let n = self.load().await?;
        Ok(v.with_rekor_min_tree_size(n))
    }
    async fn persist(&self, v: &KeylessVerifier) -> Result<(), TakoError> {
        self.save(v.rekor_max_tree_size()).await
    }
}
```

`seed`/`persist` are default-impl methods so all concrete impls inherit
the convenience for free. `async_trait` is already a workspace dep.

#### A.2 — `JsonStateStore` impls trait

The Phase 10.A inherent sync `load` / `save` / `seed` / `persist`
methods (public surface in v0.11.0+) stay unchanged. A new
`#[async_trait] impl StateStore for JsonStateStore` block delegates to
the inherent sync methods directly — file ops are sub-millisecond, no
`spawn_blocking` justified.

Purely additive: the trait impl is new; existing callers (rust + Python
facade) keep using the inherent methods unchanged.

#### A.3 — `RedisStateStore` (new, gated behind `redis` feature)

New module:
[crates/tako-governance/src/sigstore_state_redis.rs](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state_redis.rs)
mirrors [crates/tako-runtime/src/budget_redis.rs](/Users/kwc/tako-ai-core/crates/tako-runtime/src/budget_redis.rs):

- `pub struct RedisStateStore { manager: ConnectionManager, key: String, save_script: Script }`,
  with `Clone` + manual `Debug`.
- `pub async fn connect(url: &str) -> Result<Self, TakoError>` — opens
  a multiplexed `ConnectionManager`. Errors surface as
  `TakoError::Transport`.
- `pub fn with_key(self, key: impl Into<String>) -> Self` — overrides
  the default key `"tako:sigstore:rekor_min_tree_size"`.
- `load`: `GET <key>`; missing → `Ok(0)` (matches `JsonStateStore`
  first-boot semantics so the verifier's "uninitialised, no
  constraint" sentinel survives).
- `save`: invokes a Lua script enforcing **monotonic** write —
  cross-process analogue of `KeylessVerifier::rekor_max_tree_size`'s
  in-process `fetch_max`. A slow replica cannot clobber a higher
  water-mark with a stale value:

  ```lua
  local cur = tonumber(redis.call('GET', KEYS[1])) or 0
  if tonumber(ARGV[1]) >= cur then
      redis.call('SET', KEYS[1], ARGV[1])
      return ARGV[1]
  else
      return tostring(cur)
  end
  ```

- **No TTL** — unlike daily-bucketed `RedisBudgetBackend`, the Rekor
  anchor is permanent state.

`Cargo.toml` plumbing:
- [crates/tako-governance/Cargo.toml](/Users/kwc/tako-ai-core/crates/tako-governance/Cargo.toml):
  `redis = { workspace = true, optional = true }` + `[features]
  redis = ["dep:redis"]`.
- [crates/tako-governance/src/lib.rs](/Users/kwc/tako-ai-core/crates/tako-governance/src/lib.rs):
  `#[cfg(feature = "redis")] mod sigstore_state_redis;` and a public
  re-export.

#### A.4 — Python facade `RedisStateStore`

- [crates/tako-py/Cargo.toml](/Users/kwc/tako-ai-core/crates/tako-py/Cargo.toml):
  `redis = ["tako-runtime/redis", "tako-governance/redis"]` (extends
  the existing umbrella).
- [crates/tako-py/src/py_sigstore.rs](/Users/kwc/tako-ai-core/crates/tako-py/src/py_sigstore.rs):
  new `PyRedisStateStore` class mirroring `PyJsonStateStore`, gated
  `#[cfg(feature = "redis")]`. Async construction via a `connect()`
  staticmethod returning a coroutine through
  `pyo3_async_runtimes::tokio::future_into_py`. `load` / `save` /
  `seed` / `persist` exposed as coroutines too.
- [crates/tako-py/src/lib.rs](/Users/kwc/tako-ai-core/crates/tako-py/src/lib.rs):
  registers the class under `#[cfg(feature = "redis")]`.
- [python/tako/sigstore.py](/Users/kwc/tako-ai-core/python/tako/sigstore.py):
  `RedisStateStore` wrapper class with async methods, added to
  `__all__`.
- [python/tako/_native.pyi](/Users/kwc/tako-ai-core/python/tako/_native.pyi):
  matching async stubs.

Both stores ship as siblings — operator picks based on deployment
topology. The trait makes them interchangeable behind
`Arc<dyn StateStore>` for orchestrator code that wants either.

### Tests

- `crates/tako-governance/src/sigstore_state.rs` test module: a
  `fn assert_send_sync<T: StateStore>() {}` smoke proving the trait
  surface compiles and `JsonStateStore` implements it.
- `crates/tako-governance/src/sigstore_state_redis.rs` `#[cfg(test)]`:
  `#[ignore]` integration tests against `redis://localhost:6379`
  (matches `RedisBudgetBackend`'s test gating). Cover construction,
  `save(7); load() == 7`, monotonic regression
  (`save(10); save(5); load() == 10`), `with_key` override.
- `tests/python/test_sigstore_redis.py` (new): gated on
  `os.environ.get("TAKO_REDIS_TESTS")`. Smoke + monotonic regression
  through the Python facade.

## B. Streaming-aware `Verifier` (Trinity only)

### What ships

#### B.1 — `Verifier::evaluate_streaming` default-impl method

[crates/tako-core/src/traits/verifier.rs](/Users/kwc/tako-ai-core/crates/tako-core/src/traits/verifier.rs)
gains a default-impl method on the existing `Verifier` trait, mirroring
`ConfidenceGuard::evaluate_streaming`:

```rust
async fn evaluate_streaming(
    &self,
    _principal: &Principal,
    _partial: &str,
) -> Result<Option<f32>, TakoError> {
    Ok(None)
}
```

- `Ok(None)` (default) — orchestrator skips the streaming hook; the
  authoritative synthesis-complete `score()` call still fires at the
  end. Existing impls (`AlwaysScore`, downstream user impls) inherit
  this default and are zero-effort backwards-compatible.
- `Ok(Some(score))` — the orchestrator emits an `OrchEvent::VerifierScore`
  with the partial score.
- Cost note in doc-comment: don't override for LLM-as-judge verifiers;
  intended for cheap heuristics (regex, length).

The same `OrchEvent::VerifierScore { step, branch, score }` event
variant carries both partial and final scores — consumers distinguish
by `(step, branch)` repetition (multiple = streaming partials, last =
synthesis-complete final).

#### B.2 — Trinity streaming wiring

[crates/tako-orchestrator/src/trinity.rs](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/trinity.rs)
inside the SSE-streaming accumulation loop, after each text-delta
appends to the per-role buffer:

```rust
if let Some(v) = verifier.as_ref() {
    if let Some(score) = v.evaluate_streaming(&principal, &text).await? {
        let branch = role_order.iter().position(|r| r == &role).unwrap_or(0) as u32;
        yield OrchEvent::VerifierScore {
            step,
            branch,
            score: score.clamp(0.0, 1.0),
        };
    }
}
```

The Phase 10.C synthesis-complete `VerifierScore` emission stays — it
remains the authoritative final score per role per step. Streaming
partials are interleaved before it.

**No new builder knobs on `Trinity`.** Throttling lives inside the
user's `Verifier::evaluate_streaming` body via local state, exactly as
`LlmJudgeGuard::with_streaming_min_chars` lives on the guard rather
than on `SelfCaller`.

### Tests

- `crates/tako-orchestrator/tests/trinity_streaming_verifier.rs`
  (new): fake `Verifier` whose `evaluate_streaming` increments an
  `AtomicU32` and returns `Ok(Some(0.5))`. Drive Trinity with the
  existing deterministic stub-provider pattern. Assert: counter ≥ 1;
  partial events outnumber final; exactly one final score per role
  per step.
- Regression in the same file: a `Verifier` without an
  `evaluate_streaming` override emits exactly the existing single
  synthesis-complete event — proves no behaviour change.
- `tests/python/test_trinity_streaming_verifier.py` (new): Python
  `Verifier` subclass overriding `evaluate_streaming`; run
  `tako.Trinity(...).stream(...)`; assert streaming `VerifierScore`
  events surface through the SSE bridge.

## Out of scope (deferred)

Carry-forward from [PLAN.md](/Users/kwc/tako-ai-core/PLAN.md):

- **Vision / image content support across providers** — Anthropic /
  Vertex / Bedrock cross-cutting effort.
- **Eval harness real graders** (SWE-Bench Lite, GPQA Diamond) —
  sandboxed runner needed.
- **`tako-compat` real auth providers** — Vault / JWT / OIDC.
- **Streaming-aware `Verifier` in Conductor** — Conductor's
  `dispatch_workers().await` is non-streaming today. Adding a
  streaming-verifier hook requires refactoring worker dispatch to
  surface intra-worker deltas mid-flight; deferred to a future phase
  when streaming worker dispatch lands.

## Incidental fix in 13.0

[python/tako/__init__.py](/Users/kwc/tako-ai-core/python/tako/__init__.py)
and [tests/python/test_smoke.py](/Users/kwc/tako-ai-core/tests/python/test_smoke.py)
were stuck at `0.12.0` (Phase 12 oversight). Phase 13 sweeps both to
`0.14.0` along with the `Cargo.toml` workspace bump and `pyproject.toml`.
