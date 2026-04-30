# PLAN — Phase 14 (Streaming-aware Verifier in Conductor + tako-compat real auth providers)

## Context

Phase 13 (v0.14.0, 2026-04-30) shipped the
[`StateStore`](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state.rs)
trait + `RedisStateStore` impl, and the
[`Verifier::evaluate_streaming`](/Users/kwc/tako-ai-core/crates/tako-core/src/traits/verifier.rs#L73-L79)
default-impl method wired into [`Trinity::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/trinity.rs#L395).
The Phase 13.B writeup
[explicitly deferred](/Users/kwc/tako-ai-core/CHANGELOG.md) Conductor
extension because Conductor's
[`dispatch_workers_static`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L651-L757)
calls `provider.chat()` per worker (non-streaming) and returns a flat
`Vec<WorkerResult>` — there is no intra-worker delta exposure, so
per-delta verifier scoring is impossible without refactoring the
fanout. Phase 14.A closes that loop. Phase 14.B clears the long-deferred
[`tako-compat` auth carry-forward](/Users/kwc/tako-ai-core/PLAN.md):
[`StaticTokens`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth.rs)
is the only `AuthResolver` impl shipped today, but production
deployments need Vault / JWT / OIDC.

Both items are strictly additive — public APIs unchanged shape.

**Theme:** *Streaming-verifier parity across orchestrators + production-grade compat auth.*

**Target tag:** v0.15.0.

## A. Streaming-aware `Verifier` in `Conductor::stream`

### What ships

#### A.1 — Per-worker streaming dispatch with delta callback

[crates/tako-orchestrator/src/conductor.rs](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs)
gains an internal `WorkerStreamEvent` enum and a new
`dispatch_workers_streaming` free function that exposes per-worker
delta callbacks via a `tokio::sync::mpsc::UnboundedSender`:

```rust
pub(crate) enum WorkerStreamEvent {
    Delta { branch: u32, cumulative: String },
    Done  { branch: u32, result: WorkerResult },
}

async fn dispatch_workers_streaming(
    workers: &HashMap<String, Arc<dyn LlmProvider>>,
    principal: &Principal,
    plan: Vec<WorkerDispatch>,
    sem: Arc<Semaphore>,
    step: u32,
    timeout_dur: Duration,
    budget: Option<Arc<BudgetTracker>>,
    tx: mpsc::UnboundedSender<WorkerStreamEvent>,
) { /* spawns one task per worker; each task posts Delta + Done */ }
```

- Each per-worker task, after acquiring its semaphore permit, branches
  on `provider.capabilities().supports_streaming`:
  - **streaming** — call `provider.stream(...)`. For each
    `ChatChunk::Delta { text: Some(t), .. }` with `!t.is_empty()`,
    push `Delta { branch, cumulative: text.clone() }` into the channel
    after extending the cumulative buffer. On `ChatChunk::End`, send
    `Done { branch, result: Ok(...) }`. On `ChatChunk::Error`, send
    `Done { branch, result: Err(...) }`. Same semantics as
    [`Trinity::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/trinity.rs#L490-L596).
  - **non-streaming** — fall back to `provider.chat(...)`. Zero
    `Delta`s, one `Done`. This preserves existing behaviour when the
    worker provider does not advertise streaming (no per-delta hook
    fires; final synthesis-complete `score()` still emits in the outer
    loop).
- Branch identity (`(idx + 1) as u32`) is stamped at task construction
  time and travels with the worker; permit-acquisition order does not
  affect it.
- Budget pre/record bookkeeping per worker is preserved exactly as
  in `dispatch_workers_static`.
- Failed-worker outcomes (`unknown worker`, `semaphore closed`,
  timeout, provider error, finish-reason mismatch) are still surfaced
  through `WorkerResult::outcome = Err(...)` and a single `Done`.

The existing `dispatch_workers_static` is rewritten on top of
`dispatch_workers_streaming`: it spins up an mpsc, drains it,
discarding `Delta`s and collecting `Done`s in `branch` order — exactly
the v0.14.0 behaviour. `Conductor::run` (non-streaming path) keeps
calling `dispatch_workers` and sees no change.

#### A.2 — Wire into `Conductor::stream`

[`Conductor::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L396-L624)
swaps the `dispatch_workers_static(...).await` line for an mpsc-driven
recv-loop:

```rust
let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
let dispatch_count = plan.dispatch.len();
let dispatch_handle = tokio::spawn(dispatch_workers_streaming(
    workers.clone(), principal.clone(), plan.dispatch.clone(),
    Arc::clone(&semaphore), step, worker_timeout, budget.clone(), tx,
));
let mut results: Vec<Option<WorkerResult>> = vec![None; dispatch_count];
let mut done_count = 0_usize;
while let Some(evt) = rx.recv().await {
    match evt {
        WorkerStreamEvent::Delta { branch, cumulative } => {
            if let Some(v) = verifier.as_ref() {
                if let Some(score) = v.evaluate_streaming(&principal, &cumulative).await? {
                    yield OrchEvent::VerifierScore {
                        step, branch, score: score.clamp(0.0, 1.0),
                    };
                }
            }
        }
        WorkerStreamEvent::Done { branch, result } => {
            results[(branch - 1) as usize] = Some(result);
            done_count += 1;
            if done_count == dispatch_count { break; }
        }
    }
}
let _ = dispatch_handle.await;
let results: Vec<WorkerResult> = results.into_iter().flatten().collect();
```

The existing per-worker `ToolCallResult` + Phase 10.C
synthesis-complete `VerifierScore` loop at
[conductor.rs:545-580](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L545-L580)
is preserved verbatim downstream — Phase 14 only inserts streaming
partials *before* it.

**Why `mpsc::unbounded`** rather than `futures::stream::select_all`:
composes cleanly with the existing `Semaphore::acquire_owned` permit
ownership and the per-worker `JoinSet`/timeout fallback, mirrors the
mpsc bridges already used in the workspace, and the bounded
`worker_timeout` caps queue depth in practice. Verifiers that override
`evaluate_streaming` are advised by the Phase 13.B doc-comment to
"only override for cheap heuristics" (regex / length); LLM-as-judge
should keep the default `Ok(None)`. Bounded backpressure deferred to
Phase 15+.

**Ordering invariant.** All `ToolCallStart` events for the step are
emitted *before* spawning any worker tasks → before any partial
`VerifierScore` for that step. Per-worker partials may interleave
across workers (acceptable). For each worker, partials precede its
synthesis-complete final on the same `(step, branch)`.

#### A.3 — `RuleBasedVerifier` already inherits

[`RuleBasedVerifier`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/verifiers.rs)
overrode `evaluate_streaming` in Phase 13.B; Conductor users get the
cheap heuristic per-delta hook for free. No change needed.

### Tests

- `crates/tako-orchestrator/tests/conductor_streaming_verifier.rs` (new):
  - `conductor_emits_per_delta_streaming_verifier_scores_per_worker` —
    mirrors
    [`trinity_emits_per_delta_streaming_verifier_scores`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/tests/trinity.rs#L494-L614).
    Two `StreamingFake` workers; assert
    `N_a + N_b` partials + 2 finals on stable `branch`s.
  - `conductor_default_verifier_emits_only_final_score_per_worker` —
    `AlwaysScore` (default `Ok(None)`) → exactly one `VerifierScore`
    per worker (Phase 10.C parity).
  - `conductor_no_partials_for_non_streaming_workers` — non-streaming
    `FakeProvider` workers → 0 partials, 1 final per worker.
  - `conductor_branch_index_stable_under_concurrent_completion` —
    one fast worker + one 50ms-delay worker; assert `branch` stable
    across each worker's partials and its eventual final.
- `tests/python/test_phase14_conductor_streaming_verifier.py` (new):
  Python `Verifier` subclass overriding `evaluate_streaming`; run
  `tako.Conductor(...).stream(...)` against streaming-capable Python
  workers; assert per-delta `VerifierScore` events surface through the
  bridge.

## B. tako-compat real auth providers

### What ships

#### B.1 — Cargo features and dependency choices

[crates/tako-compat/Cargo.toml](/Users/kwc/tako-ai-core/crates/tako-compat/Cargo.toml)
gains three optional features. All defaults stay off — `serve_openai`
keeps working with `StaticTokens` only, and existing users see no new
transitive deps:

```toml
[features]
jwt   = ["dep:jsonwebtoken"]
oidc  = ["dep:openidconnect", "dep:reqwest", "jwt"]
vault = ["dep:vaultrs"]

[dependencies]
jsonwebtoken  = { version = "9.3", optional = true }
openidconnect = { version = "4.0", optional = true, default-features = false, features = ["reqwest"] }
vaultrs       = { version = "0.7", optional = true }
reqwest       = { workspace = true, optional = true }
```

`jsonwebtoken 9.x` is the de-facto standard HS256/RS256/ES256 lib in
the Rust ecosystem; `openidconnect 4.x` ships discovery + JWKS
rotation; `vaultrs 0.7` ships KV v2 natively. `default-features =
false` on `openidconnect` strips its `rustls-tls` default so we don't
fight the workspace's existing `native-tls` choice in `reqwest`.

[crates/tako-py/Cargo.toml](/Users/kwc/tako-ai-core/crates/tako-py/Cargo.toml)
gains umbrella features that re-export the compat features so wheels
can opt in:

```toml
auth-jwt   = ["tako-compat/jwt"]
auth-oidc  = ["tako-compat/oidc"]
auth-vault = ["tako-compat/vault"]
```

Default `maturin develop` build does **not** include them — explicit
opt-in via `maturin build --features auth-jwt,auth-oidc,auth-vault`.

#### B.2 — Module re-org

Promote
[crates/tako-compat/src/auth.rs](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth.rs)
to a `crates/tako-compat/src/auth/` directory:
- `mod.rs` — `AuthResolver` trait + `pub use` re-exports.
- `static_tokens.rs` — existing `StaticTokens` (verbatim move).
- `#[cfg(feature = "jwt")] jwt.rs` — `JwtAuthResolver`.
- `#[cfg(feature = "oidc")] oidc.rs` — `OidcAuthResolver`.
- `#[cfg(feature = "vault")] vault.rs` — `VaultAuthResolver`.

[crates/tako-compat/src/lib.rs](/Users/kwc/tako-ai-core/crates/tako-compat/src/lib.rs)
re-exports the new types so `use tako_compat::JwtAuthResolver` works.
The `AuthResolver` trait surface is unchanged — strictly additive.

#### B.3 — `JwtAuthResolver`

Supports HS256 / RS256 / ES256 against a configured signing key:

```rust
pub struct JwtAuthResolver { /* DecodingKey, Validation, claim names */ }

impl JwtAuthResolver {
    pub fn hs256(secret: &[u8]) -> Self;
    pub fn rs256_from_pem(pem: &[u8]) -> Result<Self, TakoError>;
    pub fn es256_from_pem(pem: &[u8]) -> Result<Self, TakoError>;
    pub fn with_audience(self, aud: impl Into<String>) -> Self;
    pub fn with_issuer(self, iss: impl Into<String>) -> Self;
    pub fn with_claims(self, tenant: &str, user: &str, roles: &str) -> Self;
}
```

Default claim names: `tenant_id`, `sub`, `roles`. Errors map to
`TakoError::Invalid("jwt: ...")` so the existing
[routes.rs:215-238](/Users/kwc/tako-ai-core/crates/tako-compat/src/routes.rs#L215)
401-mapping works unchanged.

#### B.4 — `OidcAuthResolver`

Wraps `openidconnect::core::CoreClient` plus a JWKS cache:

```rust
pub struct OidcAuthResolver {
    /* IssuerUrl, ClientId (audience), Arc<RwLock<JWKS>>,
       refresh_interval, claim names */
}

impl OidcAuthResolver {
    pub async fn discover(issuer: &str, audience: &str) -> Result<Self, TakoError>;
    pub fn with_refresh_interval(self, d: Duration) -> Self;  // default 1h
}
```

`resolve()` lazy-refreshes JWKS when stale; on `InvalidSignature`,
force-refreshes once and retries (handles JWKS rotation race).
Discovery (`/.well-known/openid-configuration`) runs once at
construction; JWKS rotation is the only ongoing network I/O. tako-core
architectural rule (no I/O) is unaffected — all I/O lives in
tako-compat.

#### B.5 — `VaultAuthResolver`

Resolves bearer tokens via Vault KV v2 (`<mount>/data/<path_prefix>/<token>`):

```rust
pub struct VaultAuthResolver {
    /* VaultClient, mount, path_prefix, in-memory TTL cache */
}

impl VaultAuthResolver {
    pub fn new(addr: &str, vault_token: &str) -> Result<Self, TakoError>;
    pub fn with_mount(self, m: impl Into<String>) -> Self;       // default "secret"
    pub fn with_path_prefix(self, p: impl Into<String>) -> Self; // default "tako/tokens"
    pub fn with_cache_ttl(self, d: Duration) -> Self;            // default 60s
}
```

Vault entry shape: `{tenant_id, user_id, roles}` mapped to a
`Principal`. Cache is `Arc<RwLock<HashMap<token, (Principal, Instant)>>>`.
Vault token rotation (AppRole / k8s auth) is deferred to Phase 15+;
documented in rustdoc.

#### B.6 — Python facade

[crates/tako-py/src/py_compat.rs](/Users/kwc/tako-ai-core/crates/tako-py/src/py_compat.rs)
gains three pyclasses, each `#[cfg(feature = "auth-X")]`:
- `PyJwtAuth` — staticmethods `hs256(secret: bytes)`,
  `rs256_from_pem(pem: bytes)`, `es256_from_pem(pem: bytes)` +
  builders for issuer, audience, claim names.
- `PyOidcAuth` — async `discover(issuer, audience)` via
  `pyo3_async_runtimes::tokio::future_into_py`.
- `PyVaultAuth` — sync `new(addr, token)` + builders.

`serve_openai_py` gains an `auth: Option<Py<PyAny>>` parameter; when
present, downcast to one of the three pyclasses (or fall back to
`tokens` dict → `StaticTokens`). Error if both `tokens` and `auth` are
passed. Dev-token fallback only kicks in when both are `None`.

[python/tako/compat.py](/Users/kwc/tako-ai-core/python/tako/compat.py)
signature grows:

```python
def serve_openai(
    orch, *,
    host: str = "127.0.0.1", port: int = 8080,
    tokens: dict[str, tuple[str, str]] | None = None,
    auth: JwtAuth | OidcAuth | VaultAuth | None = None,   # new
    models: list[str] | None = None,
) -> str: ...
```

`JwtAuth` / `OidcAuth` / `VaultAuth` are added as wrapper classes
re-exported from `tako._native`. Update
[python/tako/_native.pyi](/Users/kwc/tako-ai-core/python/tako/_native.pyi).

### Tests

Rust:
- `crates/tako-compat/tests/auth_jwt.rs` — HS256 round-trip; invalid
  signature → `TakoError::Invalid`; audience mismatch → error.
  `assert_send_sync<T: AuthResolver>()` smoke.
- `crates/tako-compat/tests/auth_oidc.rs` — `#[ignore]`-gated;
  `wiremock` discovery + JWKS rotation regression.
- `crates/tako-compat/tests/auth_vault.rs` — `#[ignore]`-gated against
  dev-mode Vault on `127.0.0.1:8200`; cache TTL regression.

Python:
- `tests/python/test_phase14_jwt_auth.py` — encode token via `pyjwt`
  (dev dep), boot `serve_openai(auth=JwtAuth.hs256(...))`, hit
  `/v1/chat/completions`, assert 200.
- `tests/python/test_phase14_oidc_auth.py` — gated on `TAKO_OIDC_TESTS`.
- `tests/python/test_phase14_vault_auth.py` — gated on `TAKO_VAULT_TESTS`.

## Out of scope (deferred to Phase 15+)

- **Vision / image content support across providers** — Anthropic /
  Vertex / Bedrock cross-cutter (carry-forward).
- **Eval harness real graders** (SWE-Bench Lite, GPQA Diamond) —
  sandboxed runner needed (carry-forward).
- **Per-delta streaming verifier in AB-MCTS rollouts** — would
  require deeper refactor of the rollout sampler.
- **Vault dynamic token rotation** (AppRole / Kubernetes auth methods).
- **OIDC token introspection** (RFC 7662) — only signature validation
  lands in 14.B.
- **Composite resolvers** (e.g. mTLS + bearer chaining).
- **Bounded `mpsc` backpressure** for slow per-delta verifiers.
- **Per-tenant rate limiting** in compat — orthogonal.

## Risks / open design questions

1. **`provider.stream()` capability split.** Worker tasks now have two
   code paths (stream + chat fallback). Mitigated by pulling the
   stream/chat branch directly out of the
   [Trinity precedent](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/trinity.rs#L490-L596).
2. **`mpsc::unbounded` backpressure.** Fast worker outpacing slow
   LLM-as-judge verifier could grow the queue. Doc-comment on
   `Verifier::evaluate_streaming` already warns against LLM-as-judge
   per-delta. Bounded backpressure deferred.
3. **JWKS rotation race.** In-flight refresh could see transient
   `InvalidSignature`. Mitigation: on `InvalidSignature`, force-refresh
   once and retry — pattern from `oauth2-rs`.
4. **Vault token rotation.** `VaultClient` itself uses a static Vault
   token. Periodic re-auth (AppRole, k8s) deferred to Phase 15+;
   rustdoc must call this out.
5. **OIDC `default-features = false`.** Strips `rustls-tls` to avoid
   fighting workspace `native-tls` defaults. Documented in
   `Cargo.toml` + CHANGELOG.
6. **Branch identity under fanout overflow.** When `dispatch.len() >
   max_fanout`, workers complete out of permit-acquisition order. The
   1-based branch index is stamped at construction time and travels
   with the worker — covered by
   `conductor_branch_index_stable_under_concurrent_completion`.

## Incidental documentation flips

- [PLAN.md](/Users/kwc/tako-ai-core/PLAN.md) — add Phase 14 row;
  strike "Streaming-aware verifier in Conductor" + "tako-compat real
  auth providers"; carry the rest forward.
- [README.md](/Users/kwc/tako-ai-core/README.md) — feature matrix
  Phase 14 column; roadmap entry for v0.15.0.
- [CHANGELOG.md](/Users/kwc/tako-ai-core/CHANGELOG.md) — `## [0.15.0]`
  section mirroring the 0.14.0 structure.
- Version bumps: workspace `Cargo.toml` → `0.15.0`,
  `pyproject.toml`, `python/tako/__init__.py::__version__`,
  `tests/python/test_smoke.py` version assert.
