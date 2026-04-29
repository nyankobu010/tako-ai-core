# PLAN — Phase 8 (search streaming + transparency-log completeness)

> **Status: in progress.** Successor to [PLAN_PHASE7.md](PLAN_PHASE7.md).
> Closes the four "out of scope" items flagged in `## [0.8.0]`'s
> release notes. Target tag: **v0.9.0**.

## Context

Phase 7 (v0.8.0, 2026-04-29) shipped Rekor inclusion-proof
verification, native `SelfCaller::stream`, and the cosign
protobuf-bundle adapter. Four follow-ups were explicitly deferred
to Phase 8 in the Phase 7 plan's "Out of scope" section and
restated under "Phase 8 candidates" in the [PLAN.md](PLAN.md)
roadmap:

1. **AB-MCTS native streaming.** `AbMcts::stream` is still the
   Phase 4 stub at `crates/tako-orchestrator/src/ab_mcts.rs:315-327`
   returning a one-shot `Err(TakoError::Invalid("AbMcts streaming
   is not yet implemented; use 'run'"))`. Most design-heavy item
   in the deferred set: each rollout produces a verifier score
   that has no place in the current `OrchEvent` enum.
2. **Rekor checkpoint (`SignedNote`) verification.** Phases 6/7
   already verify the Rekor SET (per-entry signature) and the
   inclusion-proof audit path; the third leg of the
   transparency-log story is the signed checkpoint over the tree
   head — a separate signature artefact that anchors the root
   hash already used by the inclusion proof.
3. **`OrchEvent::Recursion` variant.** Phase 7 deliberately did
   not extend `OrchEvent` for `SelfCaller::stream`, leaving the
   "more `StepStart`s after a `Final`" implicit signal in place.
   Phase 8's items 1 and 4 *both* now need explicit variants on
   the wire, so the enum extension lands once for all consumers.
4. **Streaming-aware `ConfidenceGuard`.** Today
   `confidence.evaluate(...)` is called only after the inner
   iteration's full assistant text is buffered
   (`self_caller.rs:258`). For long generations a guard could
   early-abort.

While extending `OrchEvent`, we also close a quiet v0.5.0 gap:
`AbMcts` has no Python facade today
(`grep -r AbMcts crates/tako-py/src/ python/tako/` returns
nothing). Adding `tako.AbMcts(...)` lines up naturally with 8.B
so Python users get streaming AB-MCTS at the same wheel cut.

## What this phase will land

### 8.0 — Plan-doc + version

- New per-phase plan doc: this file (`PLAN_PHASE8.md`).
- `PLAN.md` phase-index table: add Phase 8 row, status
  `in progress`, then flip to `done (date)` at end of phase.
- Workspace package version: `0.8.0` → `0.9.0` across
  `Cargo.toml`, `pyproject.toml`,
  `python/tako/__init__.py`,
  `tests/python/test_smoke.py`.

### 8.A — `OrchEvent` extensibility + two new variants

`crates/tako-orchestrator/src/types.rs` (the enum definition at
`types.rs:40-64`).

Mark the enum `#[non_exhaustive]` so future additive variants
don't require another minor bump. Add two new variants used by
8.B and 8.D:

```rust
#[non_exhaustive]
pub enum OrchEvent {
    // ...existing variants unchanged...
    VerifierScore { step: u32, branch: u32, score: f32 },
    Recursion { depth: u32, confidence: f32 },
}
```

The serde tag stays `kind`; the new variants serialize as
`{"kind":"verifier_score", ...}` and `{"kind":"recursion", ...}`
respectively.

PyO3 wrapper (`crates/tako-py/src/py_orch_event.rs`): add
getters `branch`, `score`, `depth`, `confidence` returning
`None` for variants that don't carry those fields. Update the
`kind` getter's match to include the two new strings. Update
type stubs in `python/tako/_native.pyi`.

Existing stream sites that exhaustively match on `OrchEvent` —
`Trinity::stream`, `Conductor::stream`, `SingleAgent::stream`,
`SelfCaller::stream`, plus the SSE reverse-mapping in
`crates/tako-compat/src/sse.rs` — all gain a `_ => continue`
(or skip) arm since they don't emit or react to the new
variants. The compat SSE path treats unknown variants as
no-ops, preserving wire compatibility with the `openai` Python
SDK.

**Tests**: 1 unit test in `types.rs` round-tripping each new
variant through serde.

### 8.B — AB-MCTS native streaming + Python binding

`crates/tako-orchestrator/src/ab_mcts.rs`. Replace the Phase-4
stub at `ab_mcts.rs:315-327`.

Mirrors the `Trinity::stream` pattern at `trinity.rs:367-668`:
clone owned state up front (`provider`, `verifier`, `tools`,
`policy`, the budget tracker if any), build an
`async_stream::try_stream!` block.

Per iteration `i in 0..max_iterations`:

1. Yield `OrchEvent::StepStart { step: i }`.
2. Run the rollout. Where the existing `iterate(...)` calls
   `provider.chat(...)` at `ab_mcts.rs:464`, refactor into a
   private `rollout_streaming(...)` helper. If the provider's
   `supports_streaming()` is true, drive `provider.stream(...)`
   and yield each chunk as `OrchEvent::AssistantText { step: i,
   delta }`. Otherwise fall back to `chat()` + one synthetic
   `AssistantText` carrying the full text — same pattern as
   `Trinity::stream`.
3. After the rollout's text is complete, run the verifier
   (`ab_mcts.rs:393-397`) and yield
   `OrchEvent::VerifierScore { step: i, branch: branch_id,
   score }`.
4. Back-propagate (existing logic).
5. Loop on early-stop if `score >= min_confidence` (existing
   logic).

After the loop, yield exactly one `OrchEvent::Final { output:
Box::new(best_output) }` constructed from the highest-scoring
branch — matching `run`'s return value.

Refactoring strategy: extract the existing rollout body into a
free-function `rollout_static(...)` so `run` and `stream` share
the loop body, just like `Conductor::stream` did with
`dispatch_workers_static` (per CHANGELOG `## [0.4.0]`).

**PyO3** — new module `crates/tako-py/src/py_ab_mcts.rs`. Wraps
`AbMcts` with the same shape as `PyTrinity` / `PySelfCaller`:

- Constructor: `(provider, verifier, *, tools=None,
  max_iterations=16, branching_factor=3, max_steps_per_rollout=4,
  temperature=0.7, min_confidence=0.95, budget=None,
  budget_backend=None)`.
- `run(prompt, *, principal=None)` → awaitable `PyOrchOutput`.
- `run_sync(...)` sibling.
- `stream(prompt, *, principal=None)` → `PyOrchEventStream`
  (the existing pyclass from Phase 7.B).
- Verifier hand-off via a new `extract_verifier_handle` helper
  that accepts either a Rust `Verifier` impl already wrapped
  for PyO3, or a Python async callable
  `(prompt, output) -> float` adapted via the same
  `Python::attach` → `into_future` pattern `PythonProvider`
  uses.

Python facade: `python/tako/__init__.py` adds `from ._native
import AbMcts as _AbMcts` and a thin `tako.AbMcts(...)`
constructor preserving kwarg names. Type stubs in
`_native.pyi`.

**Tests**:

- Rust (`crates/tako-orchestrator/tests/ab_mcts_stream.rs`,
  new file): 4 cases.
  1. Pass-through against `FakeProvider` with streaming on:
     receive interleaved `StepStart` / `AssistantText` /
     `VerifierScore` / `Final`.
  2. Non-streaming-provider fallback yields one
     `AssistantText` per iteration.
  3. Early-stop when verifier returns ≥ `min_confidence`
     yields exactly N rollouts then `Final`.
  4. Verifier-score ordering: scores arrive after each
     rollout's `AssistantText`, never before.
- Python (`tests/python/test_ab_mcts_stream.py`, new): 2
  smoke cases — basic stream consumed by `async for`, and
  `verifier_score` event surfaces the float via the
  `OrchEvent.score` getter from 8.A.

### 8.C — Rekor checkpoint (`SignedNote`) verification

`crates/tako-governance/src/sigstore.rs`. Sibling to the
existing `verify_rekor_set` (sigstore.rs:807) and
`verify_rekor_inclusion` (sigstore.rs:1080) functions.

New struct:

```rust
pub struct RekorCheckpoint {
    pub origin: String,
    pub tree_size: u64,
    pub root_hash_b64: String,
    pub key_id: String,
    pub signature_b64: String,
}
```

`RekorEntry` gains `checkpoint: Option<RekorCheckpoint>`,
serde-default `None` so v0.8.0 bundles deserialize unchanged.

New private `verify_rekor_checkpoint(rekor_key:
&CosignVerificationKey, entry: &RekorEntry, expected_root_hex:
Option<&str>) -> Result<(), TakoError>`:

1. Parse the note body. The Rekor SignedNote text format is:
   ```
   <origin>\n<tree_size>\n<base64 root_hash>\n
   \n
   — <key_id> <base64 signature>\n
   ```
   Reconstruct the signed message as
   `format!("{origin}\n{tree_size}\n{root_hash_b64}\n\n")`.
2. Verify the signature against the pinned Rekor public key
   (same key already used for the SET — no new builder method
   on `KeylessVerifier`).
3. If `expected_root_hex` is `Some` (i.e., the entry also
   carries an `inclusion_proof`), assert
   `hex::encode(b64_decode(&root_hash_b64)) ==
   expected_root_hex` so the checkpoint and the inclusion
   proof agree on the tree head.

Wire into `verify_bundle` at `sigstore.rs:497-506`: when SET
passes AND `entry.checkpoint.is_some()`, also call
`verify_rekor_checkpoint(...)`. Implicit-on-when-present,
matching the inclusion-proof's existing pattern — no new
`KeylessVerifier` builder method.

**Tests**: 3 new in
`crates/tako-governance/tests/sigstore.rs::checkpoint`:

1. Round-trip — runtime-built checkpoint signed with the
   reusable `fresh_rekor_keypair()` helper from the existing
   `inclusion_proof` sub-mod, root_hash matching a
   programmatically-built 5-leaf Merkle tree.
2. Tampered checkpoint signature rejected.
3. `root_hash_b64` that disagrees with the inclusion proof's
   `root_hash_hex` rejected.

No Python facade change required (the field is pure data
inside the bundle JSON; serde handles it transparently).

### 8.D — Streaming-aware `ConfidenceGuard`

`crates/tako-core/src/traits/confidence.rs` (the trait at
line 19) gains a default method:

```rust
async fn evaluate_streaming(
    &self,
    _principal: &Principal,
    _partial: &str,
) -> Result<Option<f32>, TakoError> {
    Ok(None)  // default: skip — keep streaming, evaluate at end
}
```

Default impl is `Ok(None)`, so all four existing impls
(`AlwaysConfident`, `ConstantConfidence`, `RuleBasedGuard`,
`LlmJudgeGuard`) compile and behave unchanged. `LlmJudgeGuard`
deliberately **does not** override the streaming method —
calling out to a judge provider on every delta is a cost
disaster.

`SelfCaller::stream` (`self_caller.rs:192+`) gains:

1. A `String` accumulator buffering cumulative assistant text
   across the inner stream's `AssistantText` deltas.
2. After yielding each `AssistantText`, call
   `confidence.evaluate_streaming(&p, &accumulated).await?`. If
   the result is `Some(c)` with `c >= min_confidence`:
   - Yield `OrchEvent::Recursion { depth, confidence: c }`
     (variant from 8.A).
   - Drop the inner stream early.
   - Build an `OrchOutput` from the accumulated text + usage
     captured so far + step count, yield exactly one
     `OrchEvent::Final { output }`, return.
3. If no early-abort triggers and the inner stream's `Final`
   arrives normally, evaluate the full text via the existing
   `evaluate` (preserved as fallback) — no behaviour change for
   guards that don't override `evaluate_streaming`.
4. After every iteration boundary (whether early-abort or
   buffered evaluation), yield
   `OrchEvent::Recursion { depth, confidence: <last_score> }`
   so consumers can observe recursion depth on the wire.

`RuleBasedGuard` gets a streaming override: if accumulated
text already passes `min_chars` + regex, return `Some(1.0)`;
else `None` to keep going.

**Tests**:

- Rust (`crates/tako-orchestrator/tests/self_caller.rs`):
  2 new cases under a `streaming_guard` sub-mod.
  1. Early abort: `RuleBasedGuard { min_chars: 10 }` with a
     `FakeProvider` emitting "0123456789ABC" in 3 deltas
     truncates the inner stream after the second delta and
     yields `Recursion { confidence: 1.0 }` followed by
     `Final` with cumulative text containing at least 10 chars.
  2. Default impl preserved: `LlmJudgeGuard` (unchanged) does
     not early-abort even if its `evaluate` would return high
     confidence — confirms the default `Ok(None)` skip.
- Python: 1 new smoke test in
  `tests/python/test_self_caller_streaming_guard.py`
  exercising the early-abort path through `tako.SelfCaller`
  + `tako.guards.RuleBased`.

### 8.E — Examples, docs, version

- New `examples/23_ab_mcts_stream.py` — `tako.AbMcts(...)`
  streaming with `FakeProvider` + a deterministic
  rule-based verifier.
- New `examples/24_sigstore_checkpoint.py` — verify a
  `KeylessBundle` carrying both an inclusion proof and a
  checkpoint, asserting they agree on the tree head.
- New `examples/25_streaming_guard.py` — `RuleBasedGuard`
  early-abort against a long-running `FakeProvider`
  generation; demonstrates the `Recursion` event arriving on
  the stream.
- `CHANGELOG.md` — new `## [0.9.0]` block summarising 8.A
  through 8.D under `### Added` / `### Changed`. Record
  `OrchEvent` becoming `#[non_exhaustive]` under `### Changed`
  with a brief migration note.
- `PLAN.md` phase-index table: flip Phase 8 to
  `done (2026-04-29)`, add Phase 9 candidate list.
- `python/tako/__init__.py::__version__` → `"0.9.0"`,
  `pyproject.toml` version → `"0.9.0"`,
  `Cargo.toml` workspace version → `"0.9.0"`,
  `tests/python/test_smoke.py` version assertion bumped.

## Verification (Definition of Done — Phase 8)

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p tako-governance --features "sigstore sigstore-protobuf"   # +3 checkpoint tests
cargo test -p tako-orchestrator                                          # +4 ab_mcts_stream, +2 streaming_guard
cargo test -p tako-orchestrator --features onnx                          # existing OnnxRouter still green

# Python
maturin develop --release --features "sigstore sigstore-protobuf"
pytest -q tests/python                                                   # +3 smoke tests, all green
ruff check python/ tests/python/ examples/                               # clean
ruff format --check python/ tests/python/ examples/                      # clean
mypy python/tako                                                         # clean

python -c "import tako; print(tako.__version__)"                         # → 0.9.0

# Examples (smoke-run)
python examples/23_ab_mcts_stream.py
python examples/24_sigstore_checkpoint.py
python examples/25_streaming_guard.py
```

## Acceptance gates

- `tako._native.OrchEvent.kind` accepts the strings
  `"verifier_score"` and `"recursion"`; `branch`, `score`,
  `depth`, `confidence` getters return appropriate `None` for
  variants that don't carry them.
- `async for ev in tako.AbMcts(...).stream(prompt): ...` yields
  interleaved `step_start` / `assistant_text` /
  `verifier_score` events and exactly one terminal `final`.
- `tako.sigstore.KeylessVerifier(..., rekor_public_key_pem=...)`
  round-trips a `KeylessBundle` whose `RekorEntry` carries both
  an `inclusion_proof` AND a `checkpoint`, with all three
  Rekor checks (SET + inclusion + checkpoint) passing; tampered
  checkpoint signature rejected; root-hash disagreement
  rejected.
- `tako.SelfCaller(inner, guard=RuleBased(min_chars=10), ...)
  .stream(prompt)` against a `FakeProvider` emitting a long
  deterministic text early-aborts at the first delta crossing
  10 chars and yields a `recursion` event with `confidence ==
  1.0` followed by `final`.
- `OrchEvent` becomes `#[non_exhaustive]`; existing match sites
  in `Conductor::stream`, `SingleAgent::stream`,
  `SelfCaller::stream`, `Trinity::stream`, and
  `tako-compat`'s SSE reverse-mapping all compile and pass
  without behavioural change.
- `CHANGELOG.md` `## [0.9.0]` block added; version bumped to
  `0.9.0` in all four locations.
- `PLAN_PHASE8.md` written; `PLAN.md` index flipped to
  `Phase 8 — done (date)`.

## Out of scope (intentional, with rationale)

- **Streaming-aware `LlmJudgeGuard`** — calling a judge
  provider on every delta is too costly to make default
  behaviour. The default `Ok(None)` impl preserves correctness;
  per-delta judge calls are a Phase 9 candidate behind an
  explicit opt-in (e.g. judge-every-N-deltas).
- **Rekor checkpoint trust-on-first-use / multi-checkpoint
  freshness anchoring.** This phase verifies a single
  checkpoint signature against a pinned key. Operator-pinned
  log-state continuity (refusing to verify if the new
  checkpoint's `tree_size` is smaller than a previously seen
  one) is a Phase 9 candidate, gated on whether operators
  surface that need.
- **Verifier-score event for non-AB-MCTS orchestrators.**
  `Trinity` and `Conductor` could in principle emit
  `VerifierScore` if extended with a verifier; this phase
  doesn't broaden them. The variant is on the wire and ready
  if a future phase wants it.
- **OpenAI-compat SSE relay of `verifier_score` /
  `recursion`.** The compat server treats the new variants as
  no-ops to preserve OpenAI SDK compatibility. A
  `tako-compat`-native protocol extension is a separate
  effort.

## Phase 9 (next milestone, indicative)

- Streaming `LlmJudgeGuard` with per-N-delta judge calls.
- Rekor log-state continuity / checkpoint freshness anchor.
- Native `tako-compat` protocol extension exposing
  `verifier_score` and `recursion` to non-OpenAI clients
  (Server-Sent Events with a `tako.*` event type alongside
  the OpenAI-shaped `data:` frames).
- AB-MCTS with `Trinity`-style learned routing per branch
  (router-driven branch expansion).
