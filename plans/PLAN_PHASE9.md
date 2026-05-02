# PLAN — Phase 9 (cost-aware streaming guards + transparency-log freshness + protocol completeness + router-driven AB-MCTS)

> **Status: complete (v0.10.0, 2026-04-30).** Successor to [PLAN_PHASE8.md](PLAN_PHASE8.md).
> Closes the four Phase 9 candidates listed in [PLAN.md](PLAN.md)'s
> roadmap as of v0.9.0. Target tag: **v0.10.0**.

## Context

Phase 8 (v0.9.0, 2026-04-29) shipped:

- `OrchEvent::VerifierScore { step, branch, score }` and
  `OrchEvent::Recursion { depth, confidence }` variants on a now
  `#[non_exhaustive]` enum
  (`crates/tako-orchestrator/src/types.rs`).
- Native `AbMcts::stream` + `tako.AbMcts` Python facade
  (`crates/tako-orchestrator/src/ab_mcts.rs`,
  `crates/tako-py/src/py_ab_mcts.rs`).
- Rekor checkpoint (`SignedNote`) verification — third leg of
  the transparency-log story alongside the v0.7.0 SET check and
  v0.8.0 inclusion-proof check
  (`crates/tako-governance/src/sigstore.rs:891-921`).
- Streaming-aware `ConfidenceGuard::evaluate_streaming` with a
  default `Ok(None)`; `RuleBasedGuard` overrides it for
  early-abort. `LlmJudgeGuard` deliberately keeps the default
  (per-delta judge calls would be a cost disaster).

Four follow-ups were explicitly tagged "Phase 9 candidate" in the
v0.9.0 release notes and restated under "Phase 9 candidates" in
[PLAN.md](PLAN.md):

1. **Streaming `LlmJudgeGuard`** — per-N-delta judge calls behind
   an explicit opt-in (cost-aware streaming-aware judging).
2. **Rekor log-state continuity / checkpoint freshness anchor** —
   trust-on-first-use over checkpoint `tree_size` to refuse
   rollback.
3. **Native `tako-compat` protocol extension** exposing
   `verifier_score` / `recursion` events to non-OpenAI clients
   (`tako.*` SSE event types alongside the OpenAI `data:` frames).
4. **AB-MCTS with `Trinity`-style learned routing per branch** —
   router-driven branch expansion.

All four are independent and additive. Phase 9 lands them
together with a doc-debt sweep that brings `README.md`'s feature
matrix current to Phases 7–9 (the matrix has been stuck at
Phase 6 since v0.7.0).

## What this phase will land

### 9.0 — Plan-doc + version

- New per-phase plan doc: this file (`PLAN_PHASE9.md`).
- `PLAN.md` phase-index table: add Phase 9 row, status
  `in progress`, then flip to `done (date)` at end of phase.
- Workspace package version: `0.9.0` → `0.10.0` across
  `Cargo.toml` (workspace + every per-crate `version =`),
  `pyproject.toml`, `python/tako/__init__.py`,
  `tests/python/test_smoke.py`.

### 9.A — Streaming `LlmJudgeGuard` (opt-in, cost-aware)

`crates/tako-orchestrator/src/self_caller.rs:404-477` defines
`LlmJudgeGuard` with fields `judge`, `rubric`, `budget`. Its
`evaluate(...)` impl runs the standard judge call pattern
(`pre_check` → `chat` → `record` → parse decimal). The trait's
default `evaluate_streaming → Ok(None)` is preserved today
(line 222 comment notes the cost rationale).

Phase 9 adds two opt-in builder knobs and an explicit
`evaluate_streaming` override:

```rust
pub struct LlmJudgeGuard {
    judge: Arc<dyn LlmProvider>,
    rubric: String,
    budget: Option<Arc<BudgetTracker>>,
    streaming_min_chars: usize,        // default usize::MAX
    streaming_every_n: u32,            // default 1
    streaming_call_count: Arc<AtomicU32>, // monotonic counter
}

impl LlmJudgeGuard {
    /// Minimum cumulative chars before the streaming hook will
    /// call the judge. Default `usize::MAX` keeps streaming
    /// disabled (preserves v0.9.0 behaviour).
    pub fn with_streaming_min_chars(mut self, n: usize) -> Self;
    /// Call the judge only every N invocations of
    /// `evaluate_streaming` that pass `min_chars`. Default 1.
    pub fn with_streaming_every_n(mut self, n: u32) -> Self;
}
```

`evaluate_streaming` body:

1. If `partial.len() < self.streaming_min_chars`, return
   `Ok(None)` — guard untouched.
2. `let n = self.streaming_call_count.fetch_add(1, Ordering::Relaxed) + 1`.
   (The trait method takes `&self`, so the counter is interior
   state via `Arc<AtomicU32>`.)
3. If `n % self.streaming_every_n != 0`, return `Ok(None)` —
   skipping this delta but still counting toward the next call.
4. Else, run the same judge-call body as `evaluate` (including
   `pre_check` → `chat` → `record` budget instrumentation, same
   `parse_confidence` parser), and return `Ok(Some(score))`.

`SelfCaller::stream`'s existing per-delta call to
`confidence.evaluate_streaming(...)` requires no change — the
override naturally drops in.

PyO3: `tako._native.LlmJudgeGuard.__init__` gains
`streaming_min_chars=` and `streaming_every_n=` kwargs forwarded
through `tako.guards.LlmJudge`. `_native.pyi` stub updated.

**Tests**:

- Rust (`crates/tako-orchestrator/src/self_caller.rs::tests`,
  new `streaming_judge` sub-mod): 3 cases.
  1. **Opt-in basic flow** — `LlmJudgeGuard` with
     `streaming_min_chars=20`, fed a partial that crosses 20
     chars, calls the (fake) judge exactly once and returns
     `Some(0.7)`.
  2. **No-op default** — same guard without streaming kwargs;
     `evaluate_streaming` returns `Ok(None)` and never invokes
     the judge.
  3. **Every-N counting** — `streaming_min_chars=10,
     streaming_every_n=3`; six over-threshold partials produce
     exactly two judge calls (counter values 3 and 6).
- Python (`tests/python/test_phase9_streaming_judge.py`): 1
  smoke covering kwarg acceptance + a fake-provider judge
  invoked when the partial crosses the threshold.

**Public API risk**: additive (new builder methods on
`LlmJudgeGuard`, new optional kwargs on the Python facade).

### 9.B — Rekor checkpoint freshness (trust-on-first-use)

`crates/tako-governance/src/sigstore.rs:891-921` already verifies
each Rekor checkpoint's signature + root-hash agreement with the
inclusion proof. There is no inter-call state today — every
`verify_bundle` is independent. Phase 9 adds an in-memory
freshness anchor per `KeylessVerifier` instance: each successful
checkpoint observation must have `tree_size >= previously
observed`. A shrinking tree size means the operator is being
shown a stale or forked tree and is rejected.

```rust
pub struct KeylessVerifier {
    // ...existing fields...
    rekor_min_tree_size: Arc<AtomicU64>, // 0 = uninitialised
}

impl KeylessVerifier {
    /// Seed the freshness anchor (e.g. from a persisted state).
    pub fn with_rekor_min_tree_size(mut self, n: u64) -> Self;
    /// Read the current high-water mark.
    pub fn rekor_max_tree_size(&self) -> u64;
}
```

`verify_rekor_checkpoint(...)` body extension (after the existing
signature + root-hash checks pass):

```rust
let prev = self.rekor_min_tree_size.load(Ordering::Relaxed);
if checkpoint.tree_size < prev {
    return Err(TakoError::Invalid(format!(
        "rekor checkpoint tree_size {} < previously observed {}",
        checkpoint.tree_size, prev
    )));
}
self.rekor_min_tree_size.fetch_max(
    checkpoint.tree_size,
    Ordering::Relaxed,
);
```

`AtomicU64::fetch_max` keeps the API `&self`-immutable, matches
`KeylessVerifier`'s `Send + Sync` use, and is correct under
concurrent verify calls. **Persistence is out of scope** — the
caller seeds `rekor_min_tree_size` at startup from a JSON file or
env var and persists `rekor_max_tree_size()` after each verify.
The 9.B API surface is forward-compatible with a follow-on
`JsonStateStore` helper.

PyO3: `tako._native.KeylessVerifier.__init__` gains an optional
`rekor_min_tree_size: Optional[int]` kwarg; new method
`rekor_max_tree_size() -> int`. Forward through
`tako.sigstore.KeylessVerifier`.

**Tests**: 3 new in
`crates/tako-governance/tests/sigstore.rs::checkpoint_freshness`:

1. **Monotonic ascent** — verify two bundles with `tree_size = 5`
   then `tree_size = 7`, both pass; `rekor_max_tree_size()` ==
   7.
2. **Rollback rejected** — verify `tree_size = 7`, then attempt
   `tree_size = 5`; second `verify_bundle` returns
   `Err(TakoError::Invalid(...))` containing
   `"tree_size 5 < previously observed 7"`.
3. **Seed enforced from construction** — construct
   `KeylessVerifier::default().with_rekor_min_tree_size(10)`;
   verify a `tree_size = 5` bundle; rejected on first call.

Plus 1 Python smoke
(`tests/python/test_phase9_rekor_freshness.py`) covering the
kwarg + monotonic-then-rollback path.

**Public API risk**: additive (new optional kwarg on
`KeylessVerifier`, new accessor).

### 9.C — `tako-compat` named SSE events for VerifierScore + Recursion

`crates/tako-compat/src/sse.rs:72-141`'s `event_to_payloads`
matches `OrchEvent::{AssistantText, ToolCallStart, Final}` to
OpenAI `chat.completion.chunk` JSON, dropping all other variants
(including the new Phase 8 `VerifierScore` and `Recursion`).
`crates/tako-compat/src/routes.rs:147-167` builds the SSE
response, calling `SseEvent::default().data(p)` per payload —
default SSE frames have no `event:` field so OpenAI clients see
only `data:` lines.

OpenAI clients ignore unknown `event:` names (per the SSE spec).
This is the natural sidechannel for a `tako.*` named-event
protocol extension: clients that opt in subscribe by name,
clients that don't never see them.

**Approach** — keep the OpenAI mapping pure; add a parallel
function for tako-extension events:

```rust
// crates/tako-compat/src/sse.rs
/// Map a single `OrchEvent` to zero or more named tako.* SSE
/// extensions (`event:` name + JSON payload). Default for all
/// variants except `VerifierScore` and `Recursion` is empty.
pub fn event_to_tako_extensions(
    event: &OrchEvent,
) -> Vec<(&'static str, String)> {
    match event {
        OrchEvent::VerifierScore { step, branch, score } => {
            let body = serde_json::json!({
                "step": step, "branch": branch, "score": score,
            });
            vec![("tako.verifier_score", body.to_string())]
        }
        OrchEvent::Recursion { depth, confidence } => {
            let body = serde_json::json!({
                "depth": depth, "confidence": confidence,
            });
            vec![("tako.recursion", body.to_string())]
        }
        _ => Vec::new(),
    }
}
```

In `routes.rs`'s SSE stream builder, after the existing
`event_to_payloads(...)` flat_map, also emit
`event_to_tako_extensions(...)` items as
`SseEvent::default().event(name).data(payload)`. Order:
**tako-extension events emit before** the OpenAI `data:` frame
for the same `OrchEvent`, so a verifier score is observable
before any related text chunk. The terminal `[DONE]` sentinel
and final assistant chunk are unchanged.

**Tests** (`crates/tako-compat/src/sse.rs::tests` +
`crates/tako-compat/tests/server.rs`):

- 2 Rust unit tests on `event_to_tako_extensions`:
  - `VerifierScore { step: 3, branch: 1, score: 0.9 }` → exactly
    one entry, name `"tako.verifier_score"`, payload
    deserialises to `{"step":3,"branch":1,"score":0.9}`.
  - `Recursion { depth: 2, confidence: 0.75 }` → exactly one
    entry, name `"tako.recursion"`.
  - All other variants (`StepStart`, `AssistantText`,
    `ToolCallStart`, `Final`) → empty `Vec`.
- 1 Rust integration test in `tests/server.rs`: runs an
  in-memory orchestrator that emits a fixed event sequence
  containing one `VerifierScore`; asserts the SSE response body
  contains `event: tako.verifier_score\ndata: {...}\n\n`
  followed later by `data: [DONE]\n\n`.

The OpenAI SDK conformance test
(`tests/python/test_compat_streaming.py`) continues to pass —
named events are silently ignored by `openai`'s SSE parser.

**Public API risk**: additive on the wire (new `event:` lines
that old clients ignore); new public Rust function
`event_to_tako_extensions` (purely additive). No breaking change.

### 9.D — AB-MCTS router-driven branch expansion

`crates/tako-orchestrator/src/ab_mcts.rs` holds a single
`provider: Arc<dyn LlmProvider>` and uses it for every rollout.
Phase 3 (`crates/tako-orchestrator/src/single.rs`) introduced the
`SingleAgent` `.candidate(p)` + `.router(r)` pattern over
`[primary, ...candidates]`; Phase 9 mirrors it on `AbMcts`, with
the router running **once per rollout** (per branch expansion).
Branch-level granularity is the natural unit for MCTS — per-step
routing inside a rollout would mask branch routing signals.

The `Router` trait at
`crates/tako-core/src/traits/router.rs:12-18` is reused
verbatim; no new types.

**Builder additions** (mirror the `SingleAgent` shape):

```rust
impl AbMctsBuilder {
    pub fn candidate(mut self, p: Arc<dyn LlmProvider>) -> Self {
        self.candidates.push(p);
        self
    }
    pub fn router(mut self, r: Arc<dyn Router>) -> Self {
        self.router = Some(r);
        self
    }
}
```

**Provider-pick helper** (free function so `run` and `stream`
share the impl, mirroring Phase 8's `rollout_static` pattern):

```rust
async fn pick_rollout_provider(
    primary: &Arc<dyn LlmProvider>,
    candidates: &[Arc<dyn LlmProvider>],
    router: Option<&Arc<dyn Router>>,
    principal: &Principal,
    messages: &[Message],
) -> Result<Arc<dyn LlmProvider>, TakoError> {
    let Some(router) = router else {
        return Ok(primary.clone());
    };
    let pool: Vec<Arc<dyn LlmProvider>> =
        std::iter::once(primary.clone())
            .chain(candidates.iter().cloned())
            .collect();
    let candidate_ids: Vec<String> =
        pool.iter().map(|p| p.id().to_string()).collect();
    let req = ChatRequest::new("router", messages.to_vec());
    let decision = router.route(principal, &req, &candidate_ids).await?;
    pool.into_iter()
        .find(|p| p.id() == decision.provider_id)
        .ok_or_else(|| TakoError::Invalid(format!(
            "router returned unknown provider id: {}",
            decision.provider_id,
        )))
}
```

**Where the router runs**: inside `iterate` (and the
`rollout_static`-equivalent path inside `stream`), once per
rollout iteration, before invoking `rollout_static(...)`. The
picked provider drives every step of that rollout. Different
branches can land on different providers; the same branch sees
consistent state across its rollout's tool-loop turns.

`rollout_static`'s signature stays the same — the caller passes
the picked provider in. No behaviour change when no router is
set.

PyO3: `tako._native.AbMcts.__init__` gains `candidates=` and
`router=` kwargs forwarded through `tako.AbMcts`. `_native.pyi`
stub updated.

**Tests**:

- Rust (`crates/tako-orchestrator/tests/ab_mcts.rs`, new
  `branch_routing` sub-mod): 3 cases, reusing `FakeProvider` and
  `RegexRouter` patterns from `tests/trinity.rs`.
  1. **Multi-provider branching** — two `FakeProvider`s
     (`"fast:code"`, `"deep:math"`), a `RegexRouter` with rules
     mapping a `code` keyword to provider 0 and `math` to
     provider 1. Run AB-MCTS with `branching_factor=2,
     max_iterations=2` against alternating prompts; assert both
     providers' call counters > 0.
  2. **No-router regression** — without `.router(...)`,
     `AbMcts::run` only invokes the primary provider (call
     counter on candidate stays at 0).
  3. **Router error propagates** — a router that returns
     `Err(TakoError::Invalid("nope"))` surfaces as a
     `Provider`-wrapped error from `AbMcts::run` and
     `AbMcts::stream`.
- Python: 1 smoke
  (`tests/python/test_phase9_ab_mcts_router.py`) covering
  kwargs + a basic two-provider routing run.

**Public API risk**: additive (new `AbMctsBuilder` methods,
new Python kwargs). `rollout_static`'s signature is internal and
private — no SemVer impact.

### 9.E — README feature matrix sweep

The README.md feature matrix
(`README.md:86-104`) stops at Phase 6 and has been stale since
v0.7.0. Phase 9 closes this in one doc commit. Add columns /
cell content for:

- **Phase 7** — streaming closures (`SelfCaller::stream`,
  `OrchEventStream` Python facade), Rekor inclusion proof,
  cosign protobuf-bundle adapter.
- **Phase 8** — AB-MCTS streaming, `OrchEvent::VerifierScore` /
  `Recursion`, Rekor checkpoint.
- **Phase 9** — AB-MCTS routing, streaming `LlmJudgeGuard`,
  Rekor checkpoint freshness, `tako-compat` named tako events.

Append the corresponding Phase 7/8/9 bullets to the README
"Roadmap" section (today it stops at Phase 6).

No tests; doc-only. Lands in the same commit as 9.0's version
bump? No — 9.E gets its own commit per the per-phase cadence
(`docs(README): feature matrix current to Phase 9`).

### 9.F — Examples + CHANGELOG + final flip

- New `examples/26_streaming_judge.py` — `LlmJudgeGuard` with
  `streaming_min_chars=80, streaming_every_n=2` against a
  `FakeProvider` long-text generation; demonstrates partial
  judge calls (mocked) and `Recursion` events on the stream.
- New `examples/27_rekor_freshness.py` — verify two bundles in
  sequence, second has a smaller `tree_size` and is rejected.
- New `examples/28_ab_mcts_router.py` — AB-MCTS with two
  candidate providers + a `RegexRouter` showing branches landing
  on different providers.
- `CHANGELOG.md` — new `## [0.10.0]` block summarising 9.A
  through 9.E under `### Added` and `### Changed`. Compare-link
  appended at the bottom.
- `PLAN.md` phase-index table: flip Phase 9 to
  `done (date)`. Add a "Phase 10 candidates" stub if any
  natural follow-ons surfaced (e.g. on-disk Rekor state
  persistence).
- `python/tako/__init__.py::__version__` already at `"0.10.0"`
  from 9.0.

## Verification (Definition of Done — Phase 9)

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p tako-orchestrator                                          # 9.A + 9.D
cargo test -p tako-governance --features "sigstore sigstore-protobuf"   # 9.B
cargo test -p tako-compat                                                # 9.C

# Python
maturin develop --release --features "sigstore sigstore-protobuf"
pytest -q tests/python                                                   # +4 smoke tests, all green
ruff check python/ tests/python/ examples/                               # clean
ruff format --check python/ tests/python/ examples/                      # clean
mypy python/tako                                                         # clean

python -c "import tako; print(tako.__version__)"                         # → 0.10.0

# Examples (smoke-run)
python examples/26_streaming_judge.py
python examples/27_rekor_freshness.py
python examples/28_ab_mcts_router.py
```

## Acceptance gates

- `LlmJudgeGuard::with_streaming_min_chars(n)` opt-in: a partial
  shorter than `n` returns `Ok(None)` from
  `evaluate_streaming`; a partial >= `n` calls the judge exactly
  once and returns `Ok(Some(score))`. Unconfigured guard remains
  `Ok(None)` for all partials.
- `KeylessVerifier::with_rekor_min_tree_size(n)` rejects any
  bundle whose checkpoint `tree_size < n`. Successive verifies
  monotonically advance `rekor_max_tree_size()`.
- An AB-MCTS streaming run that emits `VerifierScore` produces
  an `event: tako.verifier_score` SSE frame ahead of the next
  OpenAI `data:` chunk in the `tako-compat` server.
- `AbMcts::builder().candidate(p2).router(r).build()` with a
  router that maps half the prompts to `p2` exercises both
  providers across branches (call counters > 0 for both); the
  no-router builder leaves the candidate's counter at 0.
- README feature matrix shows Phase 7, 8, 9 columns / rows
  populated; README "Roadmap" enumerates Phase 7, 8, 9
  bullets.
- `CHANGELOG.md` `## [0.10.0]` block added; version bumped to
  `0.10.0` in all four locations.
- `PLAN_PHASE9.md` written; `PLAN.md` index flipped to
  `Phase 9 — done (date)`.

## Out of scope (intentional, with rationale)

- **On-disk `JsonStateStore` for Rekor freshness.** The 9.B API
  surface is forward-compatible; persistence is a follow-on
  helper that doesn't disturb the verifier itself. Operators can
  hand-roll seed/persist around `rekor_max_tree_size()` for
  v0.10.0.
- **Streaming `tako-compat` extension events for tool-call
  lifecycle.** The same `event:`-line plumbing trivially
  generalises to a `tako.tool_call` event but no consumer needs
  it yet; lands when one does.
- **AB-MCTS Trinity-style HashMap roles.** The
  `candidates+router` shape is consistent with `SingleAgent`;
  named roles add no value for an MCTS search tree where every
  branch is structurally equivalent.
- **Per-step routing inside an AB-MCTS rollout.** Branch-level is
  the right granularity; per-step would silently mask
  branch-level routing signals and violates "each branch sees
  consistent provider state".
- **Verifier-score event for non-AB-MCTS orchestrators.**
  `Trinity` and `Conductor` could in principle emit
  `VerifierScore` if extended with a verifier; this phase
  doesn't broaden them. The variant is on the wire and ready
  whenever a consumer asks.
