# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Phase 10 — Phase 9 follow-on completeness + cross-orchestrator
verifier scores + Python provider streaming. Plan:
[PLAN_PHASE10.md](PLAN_PHASE10.md).

### Added

- **On-disk `JsonStateStore` for Rekor freshness** (Phase 10.A):
  closes the v0.10.0 follow-on flagged in `## [0.10.0]`'s release
  notes. Phase 9.B shipped the in-memory anchor on
  `KeylessVerifier::with_rekor_min_tree_size` /
  `rekor_max_tree_size` but persistence was operator-rolled. Phase
  10.A adds a tiny crash-safe helper:
  - New module
    [crates/tako-governance/src/sigstore_state.rs](crates/tako-governance/src/sigstore_state.rs)
    exporting `JsonStateStore { path }` with `new`, `load` (returns
    `Ok(0)` on missing file — matches the verifier's
    "uninitialised" sentinel), `save` (atomic
    `write-temp-then-rename`), `seed(KeylessVerifier) ->
    KeylessVerifier`, and `persist(&KeylessVerifier)` convenience
    wrappers. Wire schema:
    `{ "rekor_min_tree_size": u64 }`.
  - New `&self` setter on `KeylessVerifier`:
    `set_rekor_min_tree_size(n)`, used by the PyO3 facade so the
    anchor can be applied through an `Arc<KeylessVerifier>` without
    ownership transfer. The original consuming
    `with_rekor_min_tree_size(n)` now delegates to it.
  - PyO3: `tako._native.JsonStateStore` exposes `__init__(path)`,
    `load() -> int`, `save(n: int)`, `seed(verifier) -> verifier`,
    `persist(verifier)`, and a `path()` getter. Forwarded through
    `tako.sigstore.JsonStateStore`. `_native.pyi` stub updated.
  - 5 new Rust unit tests in
    `crates/tako-governance/src/sigstore_state.rs::tests` cover
    round-trip, first-boot zero, `.tmp` non-residue after a
    successful save, missing-parent-dir auto-create, and
    parse-error surfacing. 1 new Rust integration test in
    `crates/tako-governance/tests/sigstore.rs::state_store_seed_persist`
    exercises the full seed → verify → persist cycle against the
    existing checkpoint fixture and a simulated process restart
    that rejects a smaller-tree-size bundle. 5 new Python smoke
    tests in `tests/python/test_phase10_state_store.py`.

- **Named `tako.*` SSE events for tool-call lifecycle**
  (Phase 10.B): closes the second Phase 9 follow-on. Phase 9.C
  emitted `tako.verifier_score` / `tako.recursion` named SSE
  extensions; the same mechanism now covers
  [`OrchEvent::ToolCallStart`](crates/tako-orchestrator/src/types.rs)
  and [`OrchEvent::ToolCallResult`](crates/tako-orchestrator/src/types.rs):
  - `event_to_tako_extensions` at
    [crates/tako-compat/src/sse.rs](crates/tako-compat/src/sse.rs)
    gains two new arms:
    - `ToolCallStart { step, name, id }` →
      `("tako.tool_call_start", "{\"step\":N,\"name\":...,\"id\":...}")`.
      Emitted in addition to the existing OpenAI `tool_calls`
      delta from `event_to_payloads` — OpenAI clients ignore the
      named extension per the SSE spec.
    - `ToolCallResult { step, id, result, is_error }` →
      `("tako.tool_call_result", "{\"step\":N,\"id\":...,\"result\":...,\"is_error\":...}")`.
      Closes the gap where this variant had no OpenAI mapping at
      all (silently dropped) so tako-aware clients now see tool
      results mid-stream with `is_error` propagation.
  - 3 new Rust unit tests in `sse.rs::tests`:
    `tool_call_start_emits_named_tako_extension`,
    `tool_call_result_emits_named_tako_extension`, and
    `tool_call_result_propagates_is_error_true`. The pre-existing
    `opaque_variants_emit_no_tako_extensions` regression is
    narrowed to `AssistantText` + `StepStart` (the variants that
    really do remain extension-less).
  - 1 new Rust integration test
    `stream_emits_tool_call_lifecycle_extensions` in
    `crates/tako-compat/tests/server.rs` runs a `ScriptedOrchestrator`
    that emits `ToolCallStart` then `ToolCallResult`; asserts the
    wire body contains both `event: tako.tool_call_start` and
    `event: tako.tool_call_result` lines, the result payload
    preserves the structured tool result and `is_error: false`,
    the named-start frame precedes the OpenAI `tool_calls` delta
    for the same logical event boundary, and the downstream
    assistant-text + `[DONE]` sentinel emit unchanged.

- **`OrchEvent::VerifierScore` for `Trinity` and `Conductor`**
  (Phase 10.C): the v0.9.0 enum variant has been on the wire since
  Phase 8 but only [`AbMcts`](crates/tako-orchestrator/src/ab_mcts.rs)
  emitted it. Phase 10.C adds optional verifier wiring to both
  remaining streaming orchestrators with `None` defaults, so v0.10.0
  behaviour is byte-for-byte preserved when the kwarg is omitted:
  - **`Trinity`**: new `verifier: Option<Arc<dyn Verifier>>` field
    + `TrinityBuilder::verifier(v)` builder method at
    [crates/tako-orchestrator/src/trinity.rs](crates/tako-orchestrator/src/trinity.rs).
    The streaming path emits one `OrchEvent::VerifierScore` after
    each role's assistant turn completes, with `branch` = the
    role's positional index in `role_order` so consumers can
    attribute the score to the specific role/provider that
    produced the turn.
  - **`Conductor`**: new `verifier: Option<Arc<dyn Verifier>>`
    field + `ConductorBuilder::verifier(v)` builder method at
    [crates/tako-orchestrator/src/conductor.rs](crates/tako-orchestrator/src/conductor.rs).
    The streaming path emits one `VerifierScore` per worker output
    before its result is folded back into the next coordinator
    turn, with `branch` = the 1-based worker dispatch index.
    Failed workers (whose `outcome.is_err()`) are skipped — only
    successful text outputs are scored.
  - Both call `verifier.score(principal, prompt_text, output)` at
    synthesis-complete boundaries (never per-delta); per-delta
    cost-controlled judging remains the `LlmJudgeGuard` opt-in.
    `prompt_text` is derived from `input.messages` using the same
    `filter_map(ContentPart::as_text)…join("\n")` pattern AB-MCTS
    has always used, so verifier inputs are consistent across the
    three orchestrators.
  - PyO3: `tako._native.Trinity.__init__` and
    `tako._native.Conductor.__init__` gain optional `verifier=`
    kwargs accepting any `tako._native.RuleBasedVerifier`.
    Forwarded through `tako.Trinity` / `tako.Conductor` with the
    same `verifier=` kwarg + a TypeError check
    (`"verifier must be a tako.verifiers.* instance"`). The
    `extract_any_verifier` helper in
    [crates/tako-py/src/py_ab_mcts.rs](crates/tako-py/src/py_ab_mcts.rs)
    is promoted to `pub(crate)` so all three orchestrators share
    the validation logic. `_native.pyi` stubs updated.
  - 4 new Rust integration tests (2 per orchestrator) under
    `verifier_emits` sub-mods in
    `crates/tako-orchestrator/tests/{trinity,conductor}.rs`:
    - `trinity_emits_verifier_score_when_attached` — code-prompt
      routes to the `code` role, exactly one `VerifierScore` event
      with `branch=0` and `score=0.6` (matching the `AlwaysScore`
      fixture).
    - `trinity_emits_no_verifier_score_when_unattached` — same
      setup without `.verifier(...)`, zero `VerifierScore` events.
    - `conductor_emits_verifier_score_per_worker` — coordinator
      dispatches three workers in one turn, exactly three
      `VerifierScore` events emit with `branch ∈ {1, 2, 3}` and
      `score=0.4`. All three workers' call counters confirm
      execution.
    - `conductor_emits_no_verifier_score_when_unattached` — same
      setup without `.verifier(...)`, zero `VerifierScore` events.
  - 6 new Python smoke tests across
    `tests/python/test_phase10_{trinity,conductor}_verifier.py`
    cover kwarg acceptance, TypeError on a non-verifier argument,
    and default-no-kwarg construction parity.

## [0.10.0] - 2026-04-30

Phase 9 — cost-aware streaming guards + transparency-log freshness +
protocol completeness + router-driven AB-MCTS. Closes the four
"Phase 9 candidate" follow-ups flagged in `## [0.9.0]`'s release
notes. Plan: [PLAN_PHASE9.md](PLAN_PHASE9.md).

### Added

- **Streaming-aware `LlmJudgeGuard`** (Phase 9.A): the v0.9.0
  `LlmJudgeGuard` deliberately kept the default
  `evaluate_streaming → Ok(None)` because per-delta judge calls are
  too costly to make default. Phase 9.A adds an explicit opt-in:
  - Two new builder methods at
    [crates/tako-orchestrator/src/self_caller.rs](crates/tako-orchestrator/src/self_caller.rs) —
    `with_streaming_min_chars(usize)` and
    `with_streaming_every_n(u32)`. Default `min_chars = usize::MAX`
    keeps streaming evaluation disabled (preserves v0.9.0 behaviour).
  - The `evaluate_streaming` override returns `Ok(None)` when
    `partial.len() < min_chars` or when an internal
    `Arc<AtomicU32>` counter says "skip this delta". Otherwise
    runs the same `pre_check → chat → record → parse_confidence`
    body as `evaluate` and returns `Ok(Some(score))`. Counter is
    interior so the trait method stays `&self`-immutable.
  - Refactor: judge-call body lifts into a private `run_judge`
    helper shared by `evaluate` and `evaluate_streaming`.
  - PyO3: `tako._native.LlmJudgeGuard.__init__` gains
    `streaming_min_chars=` and `streaming_every_n=` kwargs;
    forwarded through `tako.guards.LlmJudge`. Type stubs in
    `_native.pyi` updated.
  - 3 new Rust integration tests in
    `crates/tako-orchestrator/tests/self_caller.rs::streaming_judge`:
    opt-in basic flow (single judge call when partial crosses
    threshold), default-no-streaming (zero judge calls), every-N
    counting (six over-threshold partials → 2 judge calls). 2 new
    Python smoke tests in `tests/python/test_phase9_streaming_judge.py`.

- **Rekor checkpoint freshness anchor** (Phase 9.B): closes the
  third leg of the transparency-log story alongside Phase 6's SET
  check, Phase 7's inclusion proof, and Phase 8's checkpoint
  signature. Phase 9 adds a trust-on-first-use guard over the
  checkpoint's `tree_size`:
  - New field on `KeylessVerifier`:
    `rekor_min_tree_size: Arc<AtomicU64>` — high-water mark of the
    largest `tree_size` observed on this verifier instance. After
    each successful checkpoint signature + root-hash check,
    `verify_bundle` asserts `checkpoint.tree_size >= prev` and
    atomically advances the mark via `fetch_max`. A smaller value
    is rejected with `TakoError::Invalid(...)` containing the
    rollback details.
  - Two new public methods at
    [crates/tako-governance/src/sigstore.rs](crates/tako-governance/src/sigstore.rs) —
    `with_rekor_min_tree_size(u64)` (seed from a persisted state
    file at startup) and `rekor_max_tree_size() -> u64` (read back
    after each verify to write out). Persistence layer is
    intentionally out-of-band; verifier itself is in-memory.
  - PyO3: `tako._native.KeylessVerifier.__init__` gains
    `rekor_min_tree_size=` kwarg; new method
    `rekor_max_tree_size()` returns `int`. Forward through
    `tako.sigstore.KeylessVerifier`. Type stubs updated.
  - 3 new Rust tests in
    `crates/tako-governance/tests/sigstore.rs::checkpoint_freshness`:
    monotonic ascent (5 → 7 advances mark), rollback rejected
    (post-10 verify of 5 fails with clear error and leaves mark
    unchanged), seed-enforced-from-construction (post-seed-10
    verify of 5 fails on first observation). 2 Python smoke tests.
    The existing `checkpoint` mod's helpers were promoted to
    `pub(super)` for reuse.

- **Named `tako.*` SSE events for `VerifierScore` + `Recursion`**
  (Phase 9.C): the Phase 8 enum variants had no path to OpenAI-compat
  clients; the wildcard arm in
  [crates/tako-compat/src/sse.rs](crates/tako-compat/src/sse.rs)
  silently dropped them. Phase 9.C wires them to the SSE
  sidechannel that OpenAI clients ignore per the SSE spec
  (unknown `event:` lines):
  - New public function `event_to_tako_extensions(&OrchEvent) ->
    Vec<(&'static str, String)>` returns
    `("tako.verifier_score", json_payload)` for `VerifierScore`
    and `("tako.recursion", json_payload)` for `Recursion`. All
    other variants return `Vec::new()` — keeps the OpenAI mapping
    in `event_to_payloads` pure.
  - The route stream builder at
    [crates/tako-compat/src/routes.rs](crates/tako-compat/src/routes.rs)
    now emits each named extension via
    `SseEvent::default().event(name).data(payload)` BEFORE the
    related OpenAI `data:` chunk for the same `OrchEvent`, so a
    verifier score is observable ahead of any text frame.
  - 3 new unit tests in `sse.rs::tests` covering the
    VerifierScore / Recursion shapes plus a wildcard regression
    (opaque variants emit no extension). 1 new integration test in
    `tests/server.rs::stream_emits_named_tako_extension_for_verifier_score`
    asserting the wire format includes
    `event: tako.verifier_score\ndata: {...}\n\n` ahead of the
    OpenAI assistant-text chunk via a `ScriptedOrchestrator`
    fixture. The OpenAI SDK conformance test continues to pass.

- **AB-MCTS router-driven branch expansion** (Phase 9.D): closes
  the most design-heavy Phase 9 candidate. AB-MCTS held a single
  `provider: Arc<dyn LlmProvider>` and used it for every rollout;
  Phase 9 mirrors the SingleAgent `.candidate(p)` + `.router(r)`
  pattern, with the router running **once per rollout** (per
  branch expansion) — the natural granularity for an MCTS search
  tree.
  - New builder methods on `AbMctsBuilder` at
    [crates/tako-orchestrator/src/ab_mcts.rs](crates/tako-orchestrator/src/ab_mcts.rs):
    `.candidate(Arc<dyn LlmProvider>)` (additional providers the
    router may pick) and `.router(Arc<dyn Router>)`. Without
    `router`, candidates are ignored and every rollout uses the
    primary provider — backwards-compatible v0.9.0 behaviour
    (regression-tested).
  - New free helper `pick_rollout_provider` shared by `iterate`
    (the run path) and `stream`, mirroring Phase 8's
    `rollout_static` extraction pattern. Reuses the existing
    `tako_core::Router` trait verbatim — no new types.
  - PyO3: `tako._native.AbMcts.__init__` gains optional
    `candidates=` (list of provider `Py<PyAny>`) and `router=` (a
    `tako._native.RegexRouter` or `OnnxRouter`). Forward through
    `tako.AbMcts(...)` with type-checking on candidates. Type
    stubs updated.
  - 3 new Rust tests in
    `crates/tako-orchestrator/tests/ab_mcts.rs::branch_routing`:
    a `ToggleRouter` alternates across two providers and both
    counters end > 0; a no-router build leaves the candidate's
    counter at 0; a `FailingRouter`'s `Err(...)` propagates from
    `AbMcts::run`. 3 Python smoke tests covering kwarg acceptance,
    type rejection, and the no-router regression.

### Changed

- **README feature matrix + roadmap brought current to Phase 9**
  (Phase 9.E). Matrix had been stuck at Phase 6 since v0.7.0
  (Phases 7–9 lived only in CHANGELOG/PLAN). Adds Phase 7 / 8 / 9
  columns, a new "Streaming guards" row tracking the
  `evaluate_streaming` surface, and Phase 7 / 8 / 9 bullets in the
  Roadmap section.
- Workspace package version: `0.9.0` → `0.10.0` across
  `Cargo.toml`, `pyproject.toml`,
  `python/tako/__init__.py`,
  `tests/python/test_smoke.py`.
- New per-phase plan doc: `PLAN_PHASE9.md`. PLAN.md index row for
  Phase 9 flipped to `done (2026-04-30)`; Phase 10 candidate stub
  added.

### Notes

- **On-disk `JsonStateStore` for Rekor freshness** is intentionally
  out of scope: the 9.B API surface is forward-compatible with a
  follow-on helper. Operators today seed/persist around
  `rekor_max_tree_size()` from their own state layer.
- **Streaming `tako-compat` extension events for tool-call
  lifecycle** are tracked for Phase 10 — the 9.C plumbing
  trivially generalises but no consumer needs them yet.
- **Per-step routing inside an AB-MCTS rollout** stays out of
  scope — branch-level is the right granularity; per-step would
  silently mask branch routing signals and break the "consistent
  provider state inside one branch" invariant.

## [0.9.0] - 2026-04-29

Phase 8 — search streaming + transparency-log completeness. Closes
the four "out of scope" items flagged in `## [0.8.0]`'s release
notes. Plan: [PLAN_PHASE8.md](PLAN_PHASE8.md).

### Added

- **`OrchEvent::VerifierScore` and `OrchEvent::Recursion` variants**
  (Phase 8.A): two new streaming events landed alongside the
  enum's `#[non_exhaustive]` annotation. `VerifierScore { step,
  branch, score }` is consumed by AB-MCTS native streaming
  (Phase 8.B); `Recursion { depth, confidence }` is consumed by
  the streaming-aware `ConfidenceGuard` path on `SelfCaller`
  (Phase 8.D). Serde tag stays `kind`; new variants serialize
  as `{"kind":"verifier_score", ...}` and `{"kind":"recursion",
  ...}`.
- **`tako._native.OrchEvent` Python wrapper** gains four new
  getters: `branch`, `score`, `depth`, `confidence`. Each
  returns `None` for variants that don't carry the field.
  `kind` accepts the two new strings; `step` returns `Some(_)`
  on `verifier_score`. Type stubs in `_native.pyi` updated.

- **Rekor checkpoint (`SignedNote`) verification** (Phase 8.C):
  the third leg of the transparency-log story alongside the
  v0.7.0 SET check and v0.8.0 inclusion-proof check.
  - New `tako_governance::sigstore::RekorCheckpoint
    { origin, tree_size, root_hash_b64, key_id, signature_b64 }`
    struct. `RekorEntry` gains an optional
    `checkpoint: Option<RekorCheckpoint>` field (serde-default
    `None`, so v0.8.0 bundles deserialize unchanged).
  - New private `verify_rekor_checkpoint(rekor_key, checkpoint,
    expected_root_hex)` runs after the inclusion-proof check
    when both a Rekor key is pinned and the entry carries a
    checkpoint. Reconstructs the canonical signed message
    (`format!("{origin}\n{tree_size}\n{root_hash_b64}\n\n")`),
    verifies the ECDSA-P256 signature against the pinned Rekor
    key, and (when an inclusion proof is also present) asserts
    the checkpoint's `root_hash_b64` decodes to the same bytes
    as the inclusion proof's `root_hash_hex` — anchoring the
    audit path to a tree head the operator can also observe
    out-of-band.
  - 3 new Rust integration tests in
    `crates/tako-governance/tests/sigstore.rs::checkpoint`:
    round-trip with all three Rekor checks (SET + inclusion +
    checkpoint), tampered checkpoint signature rejected, and a
    *clean* root-hash-mismatch case where the checkpoint's
    signature is valid but the root disagrees with the
    inclusion proof.
  - **Implicit-on-when-present.** No new `KeylessVerifier`
    builder method — the same `with_rekor_key` already gates
    SET, inclusion-proof, and now checkpoint verification.
  - No Python facade change required (the field is pure data
    inside the bundle JSON; serde handles it transparently).

- **`tako.AbMcts` Python facade** (Phase 8.B continued): closes the
  v0.5.0 gap — AB-MCTS landed in Rust but had no Python binding.
  - New `tako._native.AbMcts(provider, verifier, *, max_iterations=,
    branching_factor=, max_steps_per_rollout=, temperature=,
    min_confidence=)` pyclass with `run`, `run_sync`, and `stream`
    methods. `stream` returns the existing `PyOrchEventStream` from
    Phase 7.B — the `verifier_score` events from 8.A surface via
    that wrapper's new `branch` and `score` getters.
  - New `tako._native.RuleBasedVerifier(min_chars=, pattern=None)`
    pyclass — the only verifier currently exposed; further verifier
    types (callable adapters, custom score fns) are tracked for
    follow-on releases.
  - Python facade: `tako.AbMcts(...)` and `tako.verifiers.RuleBased`
    (new module). Type stubs in `_native.pyi`.
  - 2 new Python smoke tests in
    `tests/python/test_ab_mcts_stream.py`: end-to-end stream against
    a `PythonProvider`-backed AB-MCTS, and verifier-score event
    branch/score-getter assertions.

- **Native `AbMcts::stream` implementation** (Phase 8.B): replaces
  the Phase 4 stub at `crates/tako-orchestrator/src/ab_mcts.rs:
  315-327` (the only orchestrator's `stream` method that was still
  returning a placeholder error).
  - Per iteration, the stream emits exactly:
    1. `OrchEvent::StepStart { step: iteration }`
    2. `OrchEvent::AssistantText { step, delta: rollout_text }`
       carrying the rollout's full text as a single delta. Per-token
       streaming inside a multi-step rollout is deferred — would
       require threading `provider.stream()` through the in-rollout
       tool-call loop, which is non-trivial and out of scope.
    3. `OrchEvent::VerifierScore { step, branch, score }` (variant
       added in 8.A) carrying the leaf's branch index and verifier
       score on `[0, 1]`.
  - `min_confidence` early-stop short-circuits the loop after the
    rollout that crosses the threshold. The stream terminates with
    exactly one `OrchEvent::Final` constructed from the
    highest-scored leaf, matching `run`'s return value.
  - Refactor: the existing rollout body lifts out of
    `AbMcts::rollout` into a free `rollout_static` function so
    `run` and `stream` share the same simulation loop.
  - 3 new Rust tests in
    `crates/tako-orchestrator/tests/ab_mcts.rs::stream`: 10-event
    happy-path round-trip with `AlwaysScore(0.5)`,
    text-before-score ordering invariant across iterations, and
    `min_confidence` early-stop yielding exactly 4 events
    (StepStart + AssistantText + VerifierScore + Final).

- **Streaming-aware `ConfidenceGuard`** (Phase 8.D): the
  trait at `tako_core::ConfidenceGuard` gains a default method
  `evaluate_streaming(&self, principal, partial: &str) ->
  Result<Option<f32>, TakoError>`. The default impl returns
  `Ok(None)` (skip — keep streaming and evaluate the buffered
  final text), so guards that don't override it behave exactly
  as before.
  - `SelfCaller::stream` now accumulates assistant text deltas
    into a per-iteration buffer and consults
    `evaluate_streaming` after each delta. If the override
    returns `Some(score)` with `score >= self.min_confidence`,
    the inner stream is dropped, an `OrchEvent::Recursion`
    event carrying the score is yielded, and a synthesised
    `OrchEvent::Final` over the accumulated text closes the
    stream. Useful for cheap rule-based heuristics.
  - `RuleBasedGuard` overrides `evaluate_streaming` to return
    `Some(1.0)` when the cumulative partial already passes
    both the length check and (when configured) the regex.
  - `LlmJudgeGuard` deliberately does **not** override the
    streaming method — calling out to a judge provider on
    every delta is a cost disaster. The default `Ok(None)`
    preserves correctness.
  - `SelfCaller::stream` also yields a new
    `OrchEvent::Recursion { depth, confidence }` event at the
    end of every iteration boundary (early-abort or buffered
    evaluation), giving consumers a first-class wire signal
    for recursion progress.
  - 2 new Rust tests in `crates/tako-orchestrator/tests/
    self_caller.rs::streaming_guard`: early-abort against a
    `StreamingFake` provider, and a control case proving the
    default `Ok(None)` path doesn't drop deltas.

### Changed

- **`OrchEvent` is now `#[non_exhaustive]`.** Pre-1.0 minor-bump
  break for downstream Rust consumers that exhaustively match
  on the enum — they need to add a wildcard arm. The Python
  facade is unaffected (the dynamic `kind`-based dispatch
  pattern never matched exhaustively).

## [0.8.0] - 2026-04-29

Phase 7 — production hardening, continued. Closes the two follow-ups
flagged in `## [0.7.0]`'s release notes plus the cosign protobuf-bundle
ergonomics carry-over tracked since v0.6.0.

### Added

- **Rekor inclusion-proof (Merkle audit-path) verification**
  (Phase 7.A): extends the v0.7.0 Rekor SET check.
  - New `tako_governance::sigstore::RekorInclusionProof
    { hashes_hex, tree_size, log_index, root_hash_hex }` struct.
    `RekorEntry` gains an optional `inclusion_proof:
    Option<RekorInclusionProof>` field (serde-default `None`, so
    v0.7.0 bundles deserialize unchanged).
  - New private `verify_rekor_inclusion(entry, proof)` runs after
    the SET check in `verify_bundle` when the entry carries a proof
    and a Rekor key is pinned. Algorithm: RFC 6962 §2.1.1 audit-path
    verification — leaf hash `SHA256(0x00 || canonicalized_body)`,
    internal hash `SHA256(0x01 || left || right)`, walk bottom-up
    per the bit-pattern of `(log_index, tree_size)`, assert the
    final hash equals the pinned `root_hash_hex`.
  - 3 new Rust integration tests in
    `crates/tako-governance/tests/sigstore.rs::inclusion_proof`:
    round-trip against a runtime-built 5-leaf Merkle tree (covers
    both the mid-tree and right-edge audit-path branches), tampered
    audit-path-hash rejected, mutated `root_hash_hex` rejected.
  - No Python facade change required — the proof is pure data
    inside the bundle JSON; serde handles the new field
    automatically.
  - **Out of scope (Phase 8 candidate)**: Rekor checkpoint
    (`SignedNote`) verification — orthogonal to the audit path
    itself.

- **Native `SelfCaller::stream` implementation** (Phase 7.B):
  replaces the Phase 4 stub at `crates/tako-orchestrator/src/
  self_caller.rs:192-202` (the only orchestrator's `stream` method
  that was still returning a placeholder error).
  - Mirrors the `Trinity::stream` pattern: clones owned state up
    front, builds an `async_stream::try_stream!` block. Each
    recursion iteration consumes the inner orchestrator's
    `BoxStream<OrchEvent>`, forwards every event verbatim, and
    intercepts `OrchEvent::Final` for the confidence-guard check.
    Only the last accepted (or max-depth) iteration's `Final` is
    yielded; intermediate `Final` events are absorbed.
  - The `OrchEvent` enum is intentionally left unchanged — the
    implicit signal "more `StepStart` events after a `Final`"
    indicates a guard rejection. A first-class
    `OrchEvent::Recursion { depth, confidence }` variant is tracked
    for Phase 8.
  - 3 new Rust tests in
    `crates/tako-orchestrator/tests/self_caller.rs`:
    pass-through-when-confident, recurse-to-max-depth-when-guard-rejects,
    AssistantText-deltas-arrive-before-Final.

- **First streaming Python entry point**
  (Phase 7.B continued): `tako.SelfCaller.stream(prompt, ...)`
  becomes the project's first async-iteration surface.
  - New `tako._native.OrchEvent` pyclass — read-only wrapper with
    a `kind` getter
    (`"step_start" | "assistant_text" | "tool_call_start" |
    "tool_call_result" | "final"`) and per-variant getters
    (`step`, `delta`, `name`, `id`, `result`, `is_error`, `text`,
    `usage`) returning `None` when the field doesn't apply.
  - New `tako._native.OrchEventStream` pyclass — async-iterable
    (`__aiter__` + async `__anext__`) over a
    `BoxStream<Result<OrchEvent>>`. The stream is parked behind a
    `tokio::sync::Mutex` so the pyclass stays `Send + Sync`.
  - `tako.SelfCaller.stream(...)` returns the stream so callers
    write `async for ev in await sc.stream(prompt): ...`. Type
    stubs added to `_native.pyi`. Future Trinity / SingleAgent
    stream bindings can reuse the shared types verbatim.
  - 2 new Python smoke tests in `test_self_caller_stream.py`.

- **cosign protobuf-bundle adapter** (Phase 7.C):
  `KeylessBundle::from_protobuf_bundle(bytes)` decodes a Sigstore
  protobuf-specs `Bundle` v1 message (the wire format of `cosign
  sign-blob --bundle out.pb`) into the JSON-shaped `KeylessBundle`
  the rest of the verifier pipeline already consumes.
  - Hand-rolled `prost::Message` types in
    `crates/tako-governance/src/cosign_bundle.rs` cover only the
    fields tako consumes. Unknown fields
    (`timestamp_verification_data`, DSSE envelopes, `kind_version`,
    Rekor checkpoints) decode as no-ops since prost ignores
    unknown tags. No `sigstore-protobuf-specs` dep, no `prost-build`
    at compile time, no `protoc` at build time.
  - Field translation: leaf cert from
    `verification_material.x509_certificate_chain.certificates[0]`
    (or `.certificate` on newer cosign builds) → `leaf_cert_pem`;
    chain → `chain_pem`; `message_signature.signature` →
    base64 → `signature_b64`; first `tlog_entries[]` →
    `Some(rekor)` including the inclusion proof from 7.A.
  - Gated behind a new `sigstore-protobuf` Cargo feature
    (depends on the existing `sigstore` feature). Default builds
    gain neither prost nor the new module.
  - 3 unit tests in `sigstore.rs::protobuf_tests`: round-trip,
    single-`certificate` form, missing-signature rejection.

- **Python facade**
  (Phase 7.C continued):
  `tako.sigstore.KeylessVerifier.verify_protobuf_bundle(manifest,
  protobuf_bundle)` — same return shape as `verify_bundle`.
  - Gated behind the new `sigstore-protobuf` feature on `tako-py`
    (forwards to the same feature on `tako-governance`); the Python
    facade raises a clear `AttributeError` when the wheel was built
    without it.
  - 3 new Python smoke tests in
    `test_phase7_sigstore_protobuf.py`.

### Changed

- Workspace package version: `0.7.0` → `0.8.0` across
  `Cargo.toml`, `pyproject.toml`, `python/tako/__init__.py`,
  `tests/python/test_smoke.py`.
- New per-phase plan docs: `PLAN_PHASE1.md` (extracted from PLAN.md
  inline body), `PLAN_PHASE4.md` (retroactive — Phase 4 had no
  per-phase doc), and `PLAN_PHASE7.md` (this phase). PLAN.md slimmed
  to a phase-index table + roadmap.

### Notes

- **Rekor checkpoint** verification (signed note over the tree
  head) remains out of scope — orthogonal to the audit path itself.
  Phase 8 candidate.
- **AB-MCTS native streaming** stays deferred to Phase 8.
- **`OrchEvent::Recursion` variant** — defer until a concrete
  consumer asks for it.
- The Phase 7.B Python streaming surface is intentionally minimal
  (events expose getters, not Python dataclasses; iteration is
  one-shot per stream). Generalising to Trinity / SingleAgent is a
  follow-on PR using the same `PyOrchEvent` /
  `PyOrchEventStream` types.

## [0.7.0] - 2026-04-29

Phase 6 — production hardening, continued. Closes the two follow-ups
flagged in `## [0.6.0]`'s release notes:

### Added

- **`BudgetTracker` wired into `Conductor`, `Trinity`, and
  `LlmJudgeGuard`** (Phase 6.A / 6.B / 6.C): mirrors the v0.6.0
  `SingleAgent` pattern across the remaining provider-call sites.
  - `Conductor::builder().budget(Arc<BudgetTracker>)` instruments
    every coordinator call and every fan-out worker call: each worker
    task runs `pre_check` → `chat` → `record` independently. A
    `BudgetExhausted` from a worker collapses into the worker's
    error outcome and is then surfaced via `fail_fast` if enabled.
  - `Trinity::builder().budget(Arc<BudgetTracker>)` instruments the
    chosen role's chat call in `run` and both the streaming and
    non-streaming paths in `stream`.
  - `LlmJudgeGuard::with_budget(Arc<BudgetTracker>)` instruments the
    judge's own provider call so a `SelfCaller` paired with an
    `LlmJudgeGuard` meters confidence-evaluation usage independently
    of the inner orchestrator's regular execution. `SelfCaller`
    itself does not grow a budget field — its `inner` orchestrator
    already carries one and direct provider calls live only in the
    guard.
  - PyO3: `tako._native.{Conductor, Trinity, LlmJudgeGuard}.__init__`
    gains `budget=` and `budget_backend=` kwargs, all routed through
    `crate::py_runtime::extract_budget_backend`. Same kwargs plumbed
    through to the Python facade in `tako.{Conductor, Trinity}` and
    `tako.guards.LlmJudge`.
  - 6 new Rust tests (3 conductor, 2 trinity, 1 self-caller) +
    3 new Python smoke tests
    (`test_phase6_budget_{conductor,trinity,judge}.py`).
  - New example `examples/19_budget_fanout.py` demonstrating budget
    tracking across a Conductor's coordinator + worker fan-out.

- **Sigstore `KeylessVerifier` chain-of-trust + Rekor SET**
  (Phase 6.D / 6.E):
  - New `tako_governance::sigstore::TrustRoot` struct, loadable
    from concatenated PEM blocks (`from_pem`) or filesystem paths
    (`from_paths`). Holds operator-pinned root + intermediate
    certificates as `Vec<x509_cert::Certificate>`.
  - `KeylessVerifier::with_trust_root(TrustRoot) -> Self` extends
    the v0.6.0 leaf-cert + identity-policy check with a
    chain-of-trust walk: each cert in the bundle's new
    `chain_pem` field is signature-validated against its issuer,
    `notBefore` / `notAfter` are checked, and the chain must
    terminate at one of the pinned roots (max 16 hops).
  - `KeylessBundle` gains two backwards-compatible fields:
    `chain_pem: Option<String>` (intermediate certs) and
    `rekor: Option<RekorEntry>` (transparency-log entry +
    SET-signed metadata). Both serde-default to `None`, so v0.6.0
    bundles deserialize unchanged.
  - `KeylessVerifier::with_rekor_key(&[u8]) -> Result<Self>` pins
    the Rekor public-good ECDSA-P256 key. When set and the bundle
    carries a `rekor` field, `verify_bundle` reconstructs the
    canonical entry JSON (sorted keys, no whitespace) and verifies
    the SET. Inclusion-proof (Merkle) verification is intentionally
    deferred to Phase 7.
  - PyO3: new `tako._native.TrustRoot` pyclass; extended
    `tako._native.KeylessVerifier` with `trust_root=` and
    `rekor_public_key_pem=` kwargs. Python facade adds
    `tako.sigstore.TrustRoot` and the matching kwargs on
    `tako.sigstore.KeylessVerifier`.
  - 4 new Rust tests (2 chain validation cases, 2 Rekor SET cases)
    + 2 new Python smoke tests in
    `tests/python/test_phase6_sigstore_chain.py`.
  - New example `examples/20_sigstore_full_chain.py` running the
    full identity + chain + Rekor pipeline against runtime-minted
    fixtures.

- Implementation uses existing deps (`x509-cert`,
  `sigstore::crypto::CosignVerificationKey`); the `sigstore` crate's
  heavy `verify` feature (with `webbrowser` + `openidconnect`) stays
  out of the dep tree.

### Notes

- `SelfCaller::stream` remains stubbed (Phase 4 carry-over). Native
  streaming is tracked for Phase 7.
- Rekor inclusion-proof (Merkle proof against the log root) is
  intentionally out of scope for v0.7.0. The `RekorEntry` JSON shape
  is forward-compatible with an added `inclusion_proof` field.
- A `cosign-bundle.json → KeylessBundle` shim is still tracked for a
  future ergonomics pass.

## [0.6.0] - 2026-04-29

Phase 5 — production hardening. Closes the three explicit follow-ups
flagged in `## [0.5.0]`'s release notes:

### Added

- **Sigstore keyless verification** (`tako_governance::KeylessVerifier`,
  Phase 5.A): a second trust model alongside the Phase-4 keyed
  `CatalogueVerifier`. The catalogue is signed by a short-lived
  Fulcio-issued leaf certificate that binds the artifact to a specific
  OIDC identity (issuer URI + SAN). Operators pin an `IdentityPolicy
  { issuer, san_match }` (where `SanMatch::Exact` or `SanMatch::Regex`)
  and call `verify_bundle(manifest, bundle)`; the verifier checks the
  cert's `notBefore` / `notAfter`, the Code Signing extended key usage,
  the OIDC issuer extension (`1.3.6.1.4.1.57264.1.1`), the SAN, and the
  signature against the cert's public key. Returns the same
  `Catalogue` shape as the keyed verifier so call sites are
  interchangeable.
- The bundle wire format (`KeylessBundle { leaf_cert_pem,
  signature_b64 }`) is a small JSON wrapper an operator can produce
  from `cosign sign-blob` output in a few lines of shell.
- Trust scope for v0.6.0 is **leaf-cert + identity-policy +
  signature**. Chain-of-trust validation against the Fulcio root and
  Rekor SET / inclusion-proof verification are explicitly deferred —
  the `verify_bundle` return shape is forward-compatible. This
  intentionally avoids the heavy `sigstore` `verify` feature
  (transitively requires `webbrowser` + `openidconnect`).
- `tako-governance` adds direct deps on `x509-cert = "0.2"` (already
  pulled in transitively by `sigstore`), `const-oid = "0.9"`, and
  `pem = "3"`, all gated behind the `sigstore` feature. Test deps add
  `rcgen` (with `aws_lc_rs` + `pem`).
- 6 Rust tests in `crates/tako-governance/tests/sigstore.rs::keyless`
  generate a Fulcio-style leaf cert at runtime (no fixtures committed):
  happy path, regex SAN, wrong issuer, wrong SAN, tampered manifest,
  malformed bundle.

- **gRPC MCP mTLS** (`tako_mcp::GrpcTransport::connect_with_tls`,
  Phase 5.B): a second constructor on the Phase-4 `GrpcTransport`
  alongside the existing plaintext / webpki-roots `connect`. Takes
  `(endpoint, ca_pem, client_cert_pem, client_key_pem, domain_name)`.
  When `client_cert_pem` and `client_key_pem` are both set, the
  transport sends a client certificate (mTLS); pass `None` for both to
  use the custom CA without client auth. Half-pair client identities
  raise synchronously with a clear error. The post-channel demux/spawn
  logic is refactored into a private `from_channel` helper shared by
  both constructors.
- 4 Rust tests in `crates/tako-mcp/tests/grpc.rs::mtls` mint a
  self-signed CA + server cert + client cert at runtime via `rcgen`
  and bind an in-process `tonic::transport::Server` with
  `ServerTlsConfig::client_ca_root`: full mTLS round-trip; server
  rejection without a client cert; CA-only round-trip without client
  auth; eager rejection of half-pair client identity.
- `tako-mcp` gains a tiny dev-dep on `rustls = "0.23"` (with the
  `aws_lc_rs` provider) so the test binary can pin a CryptoProvider —
  both `aws-lc-rs` (via rcgen) and `ring` (via tonic) end up linked,
  and rustls 0.23 refuses to auto-pick when both are present.

- **`BudgetTracker` wired into the SingleAgent orchestrator API**
  (Phase 5.C): closes the regression flagged in `## [0.5.0]` Phase 4.G
  notes. `SingleAgent` and `SingleAgentBuilder` gain an optional
  `Arc<BudgetTracker>` field plus a `.budget(...)` builder method. In
  both `Orchestrator::run` and `::stream`, every provider call is
  preceded by `pre_check(principal, estimated_usd, est_tokens)` and
  followed by `record(principal, estimated_usd, usage)`. Pre-flight
  cost uses `LlmProvider::estimate_cost_usd(&req)`; post-call cost
  reuses the same value (per-token rates aren't yet exposed on the
  trait). Pre-flight token estimate is `req.max_tokens.unwrap_or(0)`.
  `BudgetExhausted` errors short-circuit the run.
- Conductor / Trinity / SelfCaller budget wiring is intentionally
  deferred to v0.7.0 — same pattern, no public API surface disturbed.

- **Python facade for Phase-5 Rust additions**:
  - `tako.sigstore.KeylessVerifier(issuer, san, *, san_is_regex=False)`
    with `.verify_bundle(manifest, bundle)`. PyO3 binding
    `tako._native.KeylessVerifier`.
  - `tako.mcp.Grpc(endpoint, *, ca_pem=, ca_path=, client_cert_pem=,
    client_cert_path=, client_key_pem=, client_key_path=,
    domain_name=)` — accepts PEM either inline or from a filesystem
    path; the two are mutually exclusive.
  - `tako.budget.InMemoryBackend` joins `tako.budget.RedisBackend`
    with the same `current_usage` / `record` async API. Built into
    every wheel (no Cargo feature gate).
  - `tako.SingleAgent(provider, *, budget=, budget_backend=)` and
    `tako.Client(budget=, budget_backend=)` — kwargs flow through to
    the new Rust builder method.
- New PyO3 module pieces: `tako._native.InMemoryBudgetBackend`
  (always present); `tako._native.KeylessVerifier` (gated on
  `sigstore`); extended `tako._native.Grpc` constructor (gated on
  `grpc`); extended `tako._native.Orchestrator` constructor
  (`budget` / `budget_backend` kwargs).
- 12 new Python smoke tests:
  - `tests/python/test_phase5_sigstore_keyless.py` (4 cases) —
    auto-skip without `sigstore`. Generate the leaf cert via
    `cryptography` (already in the `dev` extra).
  - `tests/python/test_phase5_grpc_mtls.py` (3 cases) — auto-skip
    without `grpc`. Cover the validation rules; full mTLS round-trip
    coverage lives in the Rust integration tests.
  - `tests/python/test_phase5_budget_wiring.py` (5 cases) — always
    runs; `InMemoryBackend` round-trip, kwarg acceptance, pre-check
    short-circuit, recording usage, `Client` stashing.
- New examples: `examples/16_sigstore_keyless.py`,
  `examples/17_grpc_mtls.py`, `examples/18_budget_wired.py`.

### Notes

- Phase 5.C lands SingleAgent only. Conductor / Trinity / SelfCaller
  budget wiring is tracked for v0.7.0; the pattern is identical and
  the Python kwargs reuse the same `extract_budget_backend` helper.
- The keyless verifier's bundle JSON is intentionally simpler than
  cosign's protobuf bundle. A `--cosign-bundle` shim that converts the
  protobuf form to `KeylessBundle` is a candidate v0.7.0 ergonomics
  add.

## [0.5.0] - 2026-04-29

Phase 4 — Search & scale. Adds AB-MCTS orchestrator with verifiers
(landed pre-`[Unreleased]` against the previous tag) plus the Phase-4.D
through 4.G additions: a gRPC MCP transport, Sigstore tool-catalogue
verification, a Redis-backed `BudgetBackend`, and the matching PyO3 +
Python facade for all four. The previously-landed Phase-4.A AB-MCTS
orchestrator, Phase-4.B Mistral / Ollama providers, and Phase-4.C
WebSocket MCP transport are also published as part of this cut.

### Added

- **gRPC MCP transport** (`tako_mcp::GrpcTransport`, Phase 4.D): a fourth
  `McpTransport` impl alongside stdio, Streamable HTTP, and the Phase-4.C
  WebSocket transport. The `rmcp` crate ships no gRPC transport and the MCP
  spec doesn't standardise one, so we hand-craft a minimal JSON-RPC bridge:
  a single bidirectional streaming RPC (`tako.mcp.bridge.v1.McpBridge.Open`)
  carrying opaque `Frame { bytes json }` messages. Behaviour mirrors
  `WebSocketTransport`: a reader task spawned at `connect()` demuxes
  inbound frames into per-request `oneshot` channels (keyed by JSON-RPC
  `id`) and a `tokio::sync::broadcast` channel for server-emitted
  notifications; the outbound half is an `mpsc::Sender<Frame>` feeding
  `tonic`'s streaming request. `connect()` accepts both `http://` (plaintext)
  and `https://` (rustls + webpki-roots) endpoints; mTLS / custom CAs are
  out of scope and deferred to a later phase.
- Gated behind a new `grpc` Cargo feature on `tako-mcp` so `tonic` and the
  generated protobuf code only land in the dep tree when explicitly
  enabled. `protoc` is bundled via `protoc-bin-vendored` so contributors
  don't need a system-wide install to build with `--features grpc`; the
  `build.rs` no-ops entirely when the feature is off.
- Workspace `Cargo.toml` adds `tonic = "0.14"` (default-features off,
  `channel + codegen + router + transport + tls-ring + tls-webpki-roots`),
  `tonic-prost = "0.14"`, `tonic-prost-build = "0.14"`, `prost = "0.14"`,
  `tokio-stream = "0.1"`.
- Tests in `crates/tako-mcp/tests/grpc.rs` (4 cases, gated on `grpc`):
  happy-path JSON-RPC round-trip, 10 concurrent requests demuxed by id,
  broadcast notification fan-out, connect-error on a freed port. Server
  fixture is an in-process `tonic::transport::Server` bound to an
  ephemeral `127.0.0.1:0` port via `serve_with_incoming`.

- **Sigstore tool-catalogue verification** (`tako_governance::CatalogueVerifier`,
  Phase 4.E): an operator can pin the exact set of MCP tools a server is
  permitted to expose by signing a JSON catalogue with `cosign sign-blob`
  and shipping the catalogue + base64 signature alongside the server.
  `CatalogueVerifier::from_pem(cosign.pub)` loads the pinned key;
  `verifier.verify(manifest, signature) -> Catalogue` checks the cosign
  signature (raw or base64, ECDSA P-256 / Ed25519 / RSA) and returns the
  parsed `Catalogue { server, tools: Vec<ToolSchema> }`. The returned
  schemas pass straight to `tako_mcp::ToolRegistry::register_mcp` — no
  new coupling between `tako-governance` and `tako-mcp`.
- Trust model for this landing is **keyed** (pinned public key, the
  cosign default for `--key`); keyless verification (Fulcio cert + Rekor
  offline bundle against the Sigstore public-good trust root) is
  intentionally deferred — the same `verify` return shape will lift onto
  a bundle-based variant in a follow-up.
- Gated behind a new `sigstore` Cargo feature on `tako-governance` so
  the `sigstore` crate (and its `aws-lc-rs` crypto backend) only land in
  the dep tree when explicitly enabled.
- Workspace `Cargo.toml` adds `sigstore = "0.13"` with `default-features
  = false, features = ["cert"]` — the minimum for `CosignVerificationKey`
  + `SigStoreSigner`.
- Tests in `crates/tako-governance/tests/sigstore.rs` (6 cases, gated on
  `sigstore`): generates an ECDSA-P256 keypair at test time using
  `sigstore`'s own primitives so the fixtures are reproducible without
  `cosign` installed. Covers raw + base64 signature acceptance, tampered
  manifest detection, wrong-key rejection, malformed PEM rejection, and
  non-JSON payload rejection (after a valid signature).

- **Redis-backed `BudgetBackend`** (`tako_runtime::RedisBudgetBackend`,
  Phase 4.F): a multi-process `BudgetBackend` impl alongside the Phase-1
  `InMemoryBudgetBackend`. Keys are
  `<prefix>:{tenant_id}:{YYYY-MM-DD}` (UTC) so day rollover is automatic
  — tomorrow's writes land in a fresh key and yesterday's evicts via TTL
  (default 48 hours). `record()` is atomic via a small Lua script
  collapsing `HINCRBYFLOAT usd`, `HINCRBY tokens`, and `EXPIRE` into
  one round-trip. `current_usage()` is `HGETALL` (missing key → zero
  usage with no extra branching). `connect()` accepts both `redis://`
  (plaintext) and `rediss://` (TLS) URLs, and uses `redis::aio::ConnectionManager`
  for transparent reconnects on transient failures. `with_key_prefix`
  / `with_ttl` builder methods adjust the defaults.
- Gated behind a new `redis` Cargo feature on `tako-runtime` so the
  `redis` crate (and its TLS / async-runtime infrastructure) only land
  in the dep tree when explicitly enabled.
- Workspace `Cargo.toml` adds `redis = "1.2"` with `default-features =
  false, features = ["aio", "tokio-comp", "tokio-rustls-comp",
  "connection-manager", "script", "tls-rustls-webpki-roots"]` —
  matching the rustls + webpki-roots TLS choice used by `reqwest` and
  `tokio-tungstenite` elsewhere in the workspace. `chrono` is added as
  an optional dep on `tako-runtime` (gated by the same `redis` feature)
  for UTC day-key formatting.
- Tests in `crates/tako-runtime/tests/redis_budget.rs` (6 cases, gated
  on `redis` and auto-skipped when `REDIS_URL` is unset): missing-key
  zero-usage, record/read round-trip, multi-record accumulation,
  tenant isolation, daily-cap enforcement via `BudgetTracker`, and TTL
  application on the first record. Plus 2 unit tests in
  `src/budget_redis.rs` for the `format_day_key` pure function (date
  format stability + Unicode tenant IDs).

- **Python facade for Phase-4 Rust additions** (Phase 4.G): wires
  `WebSocketTransport`, `GrpcTransport`, `CatalogueVerifier`, and
  `RedisBudgetBackend` through to Pythonic surfaces.
  - `tako.mcp.WebSocket(url)` and `tako.mcp.Grpc(endpoint)` join the
    existing `Stdio` / `Http` transport classes; both run the
    `initialize` → `initialized` MCP handshake at construction time and
    plug into the orchestrator's heterogeneous `mcp_servers=[...]`
    arg via the extended
    `crates/tako-py/src/py_mcp.rs::extract_transport_handle`.
  - `tako.sigstore.CatalogueVerifier(pem)` (or
    `.from_pem_path(path)`) verifies a cosign-signed manifest and
    returns a `tako.sigstore.Catalogue` whose `.tools` are typed
    `tako.ToolSchema` objects ready to feed into a registry.
  - `tako.budget.RedisBackend(url, key_prefix=..., ttl_secs=...)`
    exposes the multi-process Redis budget backend with awaitable
    `current_usage(tenant_id) -> TenantUsage` and
    `record(tenant_id, usd, tokens) -> None` methods.
- New `tako-py` Cargo features: `ws`, `grpc`, `sigstore`, `redis` —
  each forwards to the matching feature on the underlying crate. The
  abi3 wheel is built with the desired subset, e.g.
  `maturin develop --features "ws grpc sigstore redis"`.
- New `crates/tako-py/src/{py_sigstore,py_runtime}.rs` modules;
  `py_mcp.rs` extended with `PyWebSocket` + `PyGrpc`.
- Python additions: new `python/tako/sigstore.py` module exporting
  `Catalogue` + `CatalogueVerifier`; `python/tako/budget.py` extended
  with `RedisBackend` + `TenantUsage`; `python/tako/mcp.py` extended
  with `WebSocket` + `Grpc`; `python/tako/_native.pyi` stubs updated.
- Tests in `tests/python/test_phase4_facades.py` (8 cases): each
  block auto-skips when its underlying class isn't on `_native` (so
  feature-stripped builds stay green). Sigstore tests use the
  `cryptography` Python library to generate an ECDSA-P256 keypair at
  test time and round-trip a signed manifest; Redis tests auto-skip
  when `REDIS_URL` is unset.
- `pyproject.toml` adds `cryptography>=43` to the `dev` extra (used
  only by the sigstore facade test; the runtime depends on neither).

### Notes

- The Python facade for `RedisBudgetBackend` exposes the backend as a
  standalone class with `record` / `current_usage`. Wiring it through
  `tako.Client` / `tako.SingleAgent` so the orchestrator
  automatically consults it is deferred — no current Python orchestrator
  surface accepts a `BudgetBackend` arg.

## [0.4.0] - 2026-04-29

Phase 3 — Learned coordination. Adds the Trinity router (rule-based +
ONNX), SelfCaller bounded-recursion wrapper, a Python training harness
+ eval harness, and replaces the Phase-2 streaming stubs in
`SingleAgent` and `Conductor` with native orchestrator-level streaming.

### Added

- **`Router` trait impls** in `tako-orchestrator`:
  - `RegexRouter`: rule-based default. Featurises the most-recent user
    message via the new shared `tako_orchestrator::features` module
    (16-dim `f32` vector) and routes through built-in code/math/fallback
    rules. `RegexRouter::builder()` accepts custom rule chains.
  - `OnnxRouter`: feature-gated behind the `onnx` Cargo feature
    (default off). Loads an ONNX classifier via `ort` 2.0.0-rc.10 with
    `load-dynamic` so the wheel stays slim. Featuriser parity with
    Python is asserted by `tests/python/test_features_parity.py`.

- **`Trinity` orchestrator** (`tako_orchestrator::Trinity`): per-turn
  role + model selection via a `Router`. Reuses the
  `HashMap<String, Arc<dyn LlmProvider>>` worker-pool shape from
  `Conductor` but with single-role-per-turn dispatch. PyO3 binding
  `tako._native.Trinity` + facade `tako.Trinity`.

- **`SelfCaller` orchestrator** (`tako_orchestrator::SelfCaller`):
  bounded-recursion wrapper over any `Arc<dyn Orchestrator>`. After
  each inner run, scores the output via `ConfidenceGuard::evaluate`;
  if below `min_confidence` AND depth `< max_depth`, recurses with a
  revision prompt appended. Depth tracked in
  `Principal.metadata["tako.recursion.depth"]` so accidental infinite
  loops are impossible.
  - `ConfidenceGuard` trait lives in `tako-core` alongside
    `AlwaysConfident` / `ConstantConfidence` test fixtures.
  - Guard impls in `tako-orchestrator`: `RuleBasedGuard` (regex +
    min-length) and `LlmJudgeGuard` (LLM-as-judge with parseable
    decimal output).
  - PyO3 bindings `tako._native.{SelfCaller, RuleBasedGuard,
    LlmJudgeGuard}` + Python facade `tako.SelfCaller` and
    `tako.guards.{RuleBased, LlmJudge}`.

- **Native orchestrator streaming** (carry-over from Phase 2.5):
  `SingleAgent::stream` and `Conductor::stream` now emit real
  `OrchEvent` streams instead of returning `Phase 2 stub` errors.
  `SingleAgent` forwards provider deltas as `OrchEvent::AssistantText`
  when the underlying provider's `supports_streaming` is true and
  falls back to `chat()` + one synthetic `AssistantText` otherwise.
  `Conductor` emits one `AssistantText` per coordinator turn plus
  `worker:<role>`-shaped `ToolCallStart` / `ToolCallResult` events for
  each dispatched worker. The `tako-compat` SSE emulation fallback is
  retained as a safety net for third-party orchestrators only.

- **Composable `Router` on `SingleAgent`**: new builder methods
  `.candidate(p)` and `.router(r)` enable per-step model selection over
  `[primary, ...candidates]` without role-switching. Backwards-compatible
  — without a router, the primary provider is used unconditionally.

- **Trinity training harness** (`python/tako/training/`):
  - `tako.training.features` — Python mirror of the Rust featuriser;
    parity asserted by a corpus test.
  - `tako.training.trinity.TrinityTrainer` — 2-layer MLP fit via numpy
    SGD. `fit_jsonl(path)` reads
    `{"prompt": ..., "label": ...}` rows; `export_onnx(path)` emits
    the model in the shape `OnnxRouter` consumes
    (`features:[1,16] → logits:[1,K]`).
  - CLI: `python -m tako.training.trinity --rollouts r.jsonl --out m.onnx`.
  - `numpy` and `onnx` are guarded by the new `tako[training]` extra so
    the base wheel stays slim.

- **Eval harness** (`python/tako/eval/`):
  - `Eval(orch, dataset, k=, concurrency=).run()` returns an
    `EvalReport` Pydantic model with pass-rate, p50/p95 latency, and
    per-task breakdowns. Phase-3 DoD requires "10-task synthetic
    benchmark + JSON report" — see
    `python/tako/eval/datasets/synthetic.jsonl` (math + factual + code
    mix).
  - `load_dataset("swe_bench_lite" | "gpqa_diamond")` raises
    `NotImplementedError` with explicit "Phase 4" pointers; no model
    weights or proprietary data committed.
  - CLI: `python -m tako.eval --orch module:fn --dataset synthetic --k 1 --out report.json`.

- **`tako._native.featurise_text(text)`** helper exposed for the
  parity test (Rust featuriser callable from Python).

- **Examples**: `13_trinity_router.py`, `14_self_caller.py`,
  `15_eval_harness.py`.

- **Docs**: new `concepts/routing.md`, `concepts/self_caller.md`,
  `recipes/trinity.md`, `recipes/self_caller.md`,
  `recipes/eval_harness.md`. `concepts/orchestrators.md` extended with
  Trinity + SelfCaller sections. mkdocs nav updated.

### Changed

- Workspace package version: `0.3.0` → `0.4.0` across `Cargo.toml`,
  `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py`.
- Workspace deps added: `ort` 2.0.0-rc.10 (default features off, `load-dynamic`
  + `ndarray`), `ndarray` 0.16. `tako-orchestrator` exposes them behind the
  `onnx` feature; `tako-py` forwards the feature.
- `tako-orchestrator` adds an `async-stream` 0.3 dep for the streaming
  generator helpers.
- `pyproject.toml` adds `[project.optional-dependencies] training = [...]`
  for the training harness's `numpy` + `onnx` deps.
- `Conductor::stream` extracts the worker-dispatch loop into a
  free-function `dispatch_workers_static` so both `run` and `stream`
  share one implementation.
- `tako._native.Orchestrator(...)` constructor adds optional
  `candidates=` and `router=` kwargs for the SingleAgent router opt-in.
- `tako._native.Trinity` accepts `roles` as a `list[tuple[str, Any]]`
  to preserve insertion order across the FFI boundary (HashMap iteration
  on the Rust side is otherwise nondeterministic).

### Deprecated

- (none)

### Removed

- `SingleAgent::stream` and `Conductor::stream` `"Phase 2"` error stubs
  — both now stream natively.

### Fixed

- `tako-orchestrator/src/single.rs` and `conductor.rs` model lookup
  now happens per-step (previously cached at the top of `run`),
  enabling per-step provider routing.

### Security

- (none)

## [0.3.0] - 2026-04-29

Phase 2.5 — cloud breadth + carry-overs. Adds Azure OpenAI and Vertex AI
(Gemini) providers; cloud secret resolvers for Vault, AWS Secrets
Manager, Azure Key Vault, and GCP Secret Manager; Bedrock streaming
(ConverseStream); OpenAI-compat SSE streaming; and a full mkdocs site
with GitHub Pages deploy.

### Added

- **Azure OpenAI provider** (`tako-providers-azure-openai`): same
  chat.completions wire format as OpenAI, but with the Azure URL shape
  (`/openai/deployments/{d}/chat/completions?api-version=...`) and
  `api-key` header auth. Provider id: `azure-openai:<deployment>`.
  PyO3 binding `tako._native.AzureOpenAi` + facade
  `tako.providers.AzureOpenAI`. 4 wiremock tests + 5 Python smoke tests.

- **Vertex AI provider** (`tako-providers-vertex`): Gemini via the
  `:generateContent` and `:streamGenerateContent?alt=sse` REST endpoints.
  Auth deferred to caller (pre-resolved OAuth2 access token via
  `.access_token()` / `.access_token_env()`); no `gcp_auth` dep added.
  Tool-call name correlation via id lookup against prior assistant
  messages. PyO3 binding `tako._native.Vertex` + facade
  `tako.providers.Vertex`. 5 wiremock tests + 5 Python smoke tests.

- **Cloud secret resolvers** in `tako-governance`:
  - `VaultResolver` (KV-v2 REST via reqwest; `path#field` JSON-pointer
    sub-key syntax).
  - `AwsSecretsManagerResolver` (`aws-sdk-secretsmanager`; deferred
    credential chain resolution; `name#version` syntax).
  - `AzureKeyVaultResolver` (REST via reqwest; deferred bearer token;
    `name#version` syntax).
  - `GcpSecretManagerResolver` (REST via reqwest; deferred bearer
    token; `name#version` syntax; base64-decodes payload).
  PyO3 bindings `tako._native.{Vault,AzureKeyVault,GcpSecretManager,
  AwsSecretsManager}Resolver` + new facade module `tako.secrets`.
  Refactor: `secrets.rs` -> `secrets/` module (mod.rs + 4 impl files).
  10 wiremock-backed Rust tests + 7 Python smoke tests.

- **Bedrock streaming**: replaces v0.2.0's `Phase 2.5` 501 stub with a
  real `ConverseStream` implementation. `stream::map_event` walks each
  event variant (MessageStart, ContentBlockStart::ToolUse,
  ContentBlockDelta::Text/ToolUse, MessageStop, Metadata) and emits
  `ChatChunk::Delta` / `End` / `Error`. Capabilities flag
  `supports_streaming` flips to `true`. 5 unit tests covering each
  branch.

- **tako-compat SSE streaming**: replaces v0.2.0's `stream=true` 501
  with a real `axum::response::sse::Sse` stream. `sse::event_to_payloads`
  reverse-maps `OrchEvent` -> OpenAI `chat.completion.chunk` JSON +
  terminal `data: [DONE]` line, matching what the official `openai`
  Python SDK consumes. When the underlying orchestrator's `stream()`
  isn't implemented, falls back to running `run()` and emulating one
  AssistantText chunk + Final — wire format is identical either way.
  4 sse unit tests + replaces the obsolete `returns_501` server
  integration test with one that asserts SSE chunks + DONE.
  `tests/python/test_compat_streaming.py` includes both a raw-SSE
  wire-format test and an `openai` SDK conformance test (skip-if-not-
  installed).

- **mkdocs site**: full nav under `docs/`:
  - `concepts/`: providers, orchestrators, policy, secrets, budgets,
    tracing, mcp.
  - `recipes/`: azure_openai, vertex, bedrock, openai_compat_server,
    conductor, opa_policy, secret_resolvers.
  - `api/`: python (mkdocstrings), rust (docs.rs links).
  Material theme with light+dark, navigation.sections, search.highlight.
  `mkdocs.yml` moves to repo root (modern Material requirement).
  `mkdocs build --strict` is clean.

- **`.github/workflows/docs.yml`**: builds the mkdocs site on push to
  main when `docs/` or `python/tako/` change, deploys to GitHub Pages
  via `actions/deploy-pages@v4`. Repo Pages source must be set to
  'GitHub Actions' once post-merge.

- Examples: `09_azure_openai.py`, `10_vertex_gemini.py`,
  `11_secrets_vault.py`, `12_bedrock_streaming.py`.

### Changed

- Workspace package version: `0.2.0` -> `0.3.0` across `Cargo.toml`,
  `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py`.
- `Bedrock` `supports_streaming` capability flips to `true`.
- `tako-providers-openai` exposes `convert` and `stream` modules as
  `#[doc(hidden)] pub mod` so the Azure OpenAI crate can reuse them.
- Workspace deps added: `aws-sdk-secretsmanager` 1.83, `base64` 0.22.
  Bedrock crate adds `async-stream` 0.3 (already a dep of openai/anthropic
  providers).
- `tako-governance/Cargo.toml` adds `reqwest`, `base64`, `aws-config`,
  `aws-sdk-secretsmanager` for cloud resolvers; `wiremock` as dev-dep.

### Deprecated

- (none)

### Removed

- The `Phase 2.5` 501 stubs in `BedrockProvider::stream()` and
  `tako-compat`'s `chat_completions` for `stream=true`. Both replaced
  with real streaming.

### Fixed

- Bedrock provider's `supports_streaming` capability incorrectly read
  `false`; flipped to `true` now that streaming works.

### Security

- (none net new — cloud resolvers all use the same SecretString
  redaction story as `EnvResolver`)

## [0.2.0] - 2026-04-29

Phase 2 + bundled Phase 1.5 follow-ups. Adds Conductor, Bedrock,
OPA/Rego enforcement, an OpenAI-compatible HTTP server, and closes the
remaining Python-parity gaps from Phase 1 (MCP transports,
`PythonProvider`, OTLP exporter).

### Added

- **Phase 1.5 — Python parity:**
  - `tako._native.Stdio(command, args)` and `tako._native.StreamableHttp(url, ...)`
    plus `tako.mcp.Stdio` / `tako.mcp.Http` Python wrappers.
  - `tako.SingleAgent(provider, mcp_servers=[...])` discovers tools at
    construction time via MCP `tools/list`.
  - `tako._native.PythonProvider(id, chat=...)` + `tako.providers.PythonProvider`:
    user-defined `LlmProvider`s in pure Python via an async callable.
    GIL-correct hand-off (`Python::attach` → `into_future` →
    await-without-GIL).
  - Real OTLP gRPC exporter via `opentelemetry-otlp` 0.31 + tonic.
    `tako.tracing.init_otlp(endpoint, ...)` + `shutdown_otlp()`. Process-
    global guard flushes pending spans on interpreter exit.

- **Phase 2 features:**
  - `tako-providers/bedrock`: Amazon Bedrock provider via the Converse
    API (`aws-sdk-bedrockruntime` 1.130). Supports text, tool calls, and
    tool results; system messages hoist to the top-level `system` field.
    Streaming (ConverseStream) is documented as Phase 2.5.
    `tako._native.Bedrock` + `tako.providers.Bedrock` Python wrappers.
  - `Conductor` orchestrator (arXiv:2512.04388 generalisation): a
    coordinator LLM emits structured dispatch JSON; workers keyed by role
    name (`code`, `math`, …) run concurrently under an `Arc<Semaphore>`
    capped at `max_fanout`. Configurable `max_steps`, `worker_timeout`,
    `fail_fast`. Markdown ` ```json ` fences are stripped; malformed
    output is fed back as a one-turn retry. `tako.Conductor(...)` Python
    wrapper.
  - `tako_governance::policy`: OPA/Rego enforcement via `regorus` 0.9.
    `OpaBundle::from_string` / `from_path` with SHA-256 source caching;
    `PolicyEngine` impl for three stages (`PreChat`, `PreTool`,
    `PostChat`). `AuditLog::jsonl(path)` + `in_memory()` writes
    every decision as JSONL. `SingleAgentBuilder::policy(...)` consults
    the engine before each tool invocation; `Deny` /
    `RequireApproval` propagate as `TakoError::PolicyDenied`.
  - `tako-compat`: OpenAI-compatible HTTP server (`axum` 0.8). Routes:
    `POST /v1/chat/completions` (non-streaming), `GET /v1/models`,
    `GET /healthz`, `GET /readyz`. Bearer-token auth via `AuthResolver`
    + `StaticTokens`. `tako._native.serve_openai_py` +
    `tako.compat.serve_openai(orch, host, port, tokens, models)`.
    Streaming SSE deferred to Phase 2.5; stream requests return 501.

- Examples: `02_conductor.py`, `07_openai_compat_server.py`.
- Python tests: `test_mcp_stdio.py`, `test_python_provider.py`,
  `test_otlp.py`, `test_conductor.py`, `test_compat_server.py`
  (now 20 Python tests; was 8 in Phase 1).
- Rust tests: 7 conductor cases, 2 policy E2E cases, 6 bedrock convert
  cases, 6 compat-server cases, 4 OPA-policy unit cases (~94 total).

### Changed

- Workspace package version: `0.1.0` → `0.2.0` across `Cargo.toml`,
  `pyproject.toml`, `python/tako/__init__.py`.
- Workspace deps added: `regorus` 0.9, `aws-config` 1.8,
  `aws-sdk-bedrockruntime` 1.130, `axum` 0.8, `tower` 0.5,
  `tower-http` 0.6, `hyper` 1.
- New workspace member: `crates/tako-compat`,
  `crates/tako-providers/bedrock`.

### Deprecated

- (none)

### Removed

- The Phase-1 placeholder `tako.tracing.Otlp` no-op was replaced with a
  config object that delegates to `init_otlp`.

### Fixed

- `tako_governance::otel::init_otlp_tracing` now actually wires an OTLP
  exporter (was a warn-and-delegate stub in Phase 1). Constructor enters
  the shared Tokio runtime handle so hyper-util doesn't panic on the
  missing reactor.

### Security

- All policy decisions through `OpaBundle` are recorded to the configured
  `AuditLog` for SIEM ingestion (JSONL: timestamp, principal, stage,
  decision, model).

## [0.1.0] - 2026-04-28

Initial Phase 1 foundation release.

### Added

- Initial workspace scaffolding for the Phase 1 foundation:
  `tako-core`, `tako-runtime`, `tako-providers/{anthropic,openai,http-generic}`,
  `tako-mcp`, `tako-orchestrator`, `tako-governance`, `tako-py`.
- Five core async traits in `tako-core`: `LlmProvider`, `Tool`, `McpTransport`,
  `Router`, `PolicyEngine`.
- `SingleAgent` orchestrator with a max-step tool-call loop.
- Anthropic Messages and OpenAI Chat Completions providers with streaming SSE
  and tool calls.
- MCP client transports: stdio (subprocess) and Streamable HTTP, via `rmcp`.
- In-memory budget tracker with a pluggable `BudgetBackend` trait.
- `failsafe`-backed circuit breaker, `governor` rate limiter, retry-with-jitter.
- OpenTelemetry pipeline emitting `tako.*` and `gen_ai.*` semconv attributes
  (stub OTLP exporter; real wiring landed in 0.2.0).
- Presidio-style PII regex content transform (mask / hash / redact).
- PyO3 bindings (`tako._native`) plus a Pydantic-v2 Python facade
  (`python/tako/`).
- Sync + async dual API: every async method has a `_sync` sibling.
- CI workflows: fmt + clippy + cargo test + maturin develop + pytest +
  cargo-audit + pip-audit on Linux/macOS/Windows.

### Changed

- Pinned crate versions to current stable as of 2026-04-28; differs from the
  spec snapshot:
  - `tokio` 1.43 → 1.52, `reqwest` 0.12 → 0.13, `governor` 0.7 → 0.10,
    `schemars` 0.8 → 1.2, `rmcp` 0.16 → 1.5, `regorus` 0.4 → 0.9,
    `sigstore` 0.10 → 0.13, `tokio-tungstenite` 0.24 → 0.29,
    `tonic` 0.12 → 0.14, `prost` 0.13 → 0.14, `ort` rc.10 → rc.12,
    `aws-sdk-bedrockruntime` 1.50 → 1.130.

### Security

- `cargo audit` and `pip-audit` integrated into CI.

[Unreleased]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.10.0...HEAD
[0.10.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/TODO(<org>)/tako-ai-core/releases/tag/v0.1.0
