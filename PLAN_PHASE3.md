# PLAN — Phase 3 (Learned coordination)

> **Status: complete (v0.4.0, 2026-04-29).** All four spec deliverables
> shipped, plus the orchestrator-streaming carry-over from Phase 2.5.
> See [CHANGELOG.md](CHANGELOG.md) `## [0.4.0]` for the full diff.
>
> Successor to [PLAN_PHASE25.md](PLAN_PHASE25.md). Phase 4 (AB-MCTS,
> Mistral/Ollama providers, WS/gRPC MCP, Sigstore, Redis budgets) is
> next; see [PLAN.md](PLAN.md).

## Context

Phases 1, 2, and 2.5 shipped a foundation, multi-orchestrator
coordination (Conductor), and cloud breadth (Azure OpenAI, Vertex,
Bedrock streaming, secret resolvers). Phase 3 implements the *learned
coordination* tier from the spec (prompt.md §10.3, §10.5, §16, §18):
Trinity router (rule + ONNX), SelfCaller bounded recursion, training
harness, and eval harness skeleton. It also closes the Phase 2.5
orchestrator-streaming gap by replacing `SingleAgent::stream` and
`Conductor::stream` `"Phase 2"` stubs with native event streams.

## What landed

### 3.A — Router impls

`crates/tako-orchestrator/src/router.rs`. Two impls of the existing
`tako_core::Router` trait:

- `RegexRouter` — rule-based default. Featurises the most-recent user
  message via the new `tako_orchestrator::features` module (16-dim
  `f32` vector, hand-tuned features over length / token-class / math
  symbols / keywords / punctuation). Built-in rules map code →
  candidate 0, math → candidate 1, fallback → `default_idx`.
- `OnnxRouter` — feature-gated behind the `onnx` Cargo feature
  (default off). Loads a classifier via `ort` 2.0.0-rc.10 with
  `load-dynamic` so `libonnxruntime.{so,dylib,dll}` is loaded at
  runtime and the wheel itself stays slim. Argmax over `[1, K]`
  logits; confidence reported as the softmax of the chosen index.

OTel: each router emits `tako.router.route` with `tako.router.kind`,
`tako.router.choice`, `tako.router.confidence`.

### 3.B — Trinity orchestrator

`crates/tako-orchestrator/src/trinity.rs`. Per-turn role + model
selection via a `Router`. Reuses the
`HashMap<String, Arc<dyn LlmProvider>>` worker-pool shape from
`Conductor`. Builder mirrors `SingleAgentBuilder` /
`ConductorBuilder`: `roles(...)`, `router(...)`, `policy(...)`,
`tools(...)`, `max_steps(...)`. Records
`tako.orchestrator.run` with `tako.orchestrator.kind = "trinity"` and
per-turn `tako.provider.chat` spans tagged with the chosen role.

### 3.C — SelfCaller orchestrator

`crates/tako-orchestrator/src/self_caller.rs`. Bounded-recursion wrapper
over any `Arc<dyn Orchestrator>`. After each inner run, scores the
output via `ConfidenceGuard::evaluate`; if below `min_confidence`
AND depth `< max_depth`, recurses with a revision prompt appended.
Depth tracked in `Principal.metadata["tako.recursion.depth"]` so
nested SelfCallers across module boundaries share one counter.

`ConfidenceGuard` trait lives in `tako-core` next to other dyn-compatible
contracts. Built-in impls in `tako-orchestrator`:

- `RuleBasedGuard` — min-length + optional regex (cheap default).
- `LlmJudgeGuard` — LLM-as-judge with parseable decimal output.

DoD §2 ("SelfCaller terminates within `max_depth` on adversarial
inputs") is pinned by
`tests/python/test_self_caller.py::test_terminates_within_max_depth_on_adversarial`.

### 3.D — Native orchestrator streaming

Replaces the `"Phase 2"` error stubs in `SingleAgent::stream` and
`Conductor::stream` with real `OrchEvent` streams emitted via
`async_stream::try_stream!`. SingleAgent forwards provider deltas as
`OrchEvent::AssistantText` when the underlying provider's
`supports_streaming` is true, and falls back to `chat()` + one
synthetic `AssistantText` otherwise. Conductor emits one
`AssistantText` per coordinator turn plus
`worker:<role>`-shaped `ToolCallStart` / `ToolCallResult` events per
dispatched worker. The `tako-compat` SSE emulation fallback is
retained as a safety net for third-party orchestrators only (in-tree
orchs no longer hit it).

### 3.E — Composable Router on SingleAgent

New builder methods `.candidate(p)` and `.router(r)` enable per-step
model selection over `[primary, ...candidates]` without role-switching.
Backwards-compatible: without a router, the primary provider is used
unconditionally.

### 3.F — Trinity training harness

`python/tako/training/`:

- `features.py` — Python mirror of `tako_orchestrator::features`.
  Byte-for-byte parity asserted by
  `tests/python/test_features_parity.py` over a 10-string corpus.
- `trinity.py` — `Rollout` data class + `TrinityTrainer` (2-layer MLP,
  16 features → hidden → K logits, fit via numpy SGD). `fit_jsonl(path)`
  reads `{prompt, label}` rows; `export_onnx(path)` emits the model in
  the `OnnxRouter`-consumable shape. CLI:
  `python -m tako.training.trinity --rollouts r.jsonl --out m.onnx`.
- `numpy` and `onnx` are guarded behind the new
  `tako[training]` extra so the base wheel stays slim.

### 3.G — Eval harness

`python/tako/eval/`:

- `harness.py` — `Eval(orch, dataset, k=, concurrency=).run()` returns
  an `EvalReport` Pydantic model with pass-rate, p50/p95 latency, and
  per-task breakdowns.
- `datasets/synthetic.jsonl` — 10-task synthetic benchmark
  (math + factual + code mix) shipped to satisfy DoD §3.
- External-dataset loaders (`swe_bench_lite`, `gpqa_diamond`) raise
  `NotImplementedError` with explicit Phase-4 pointers; no model
  weights or proprietary data committed.
- CLI: `python -m tako.eval --orch module:fn --dataset synthetic --k 1 --out report.json`.

### 3.H — PyO3 bindings + Python facade

- New pyclasses: `Trinity`, `SelfCaller`, `RuleBasedGuard`,
  `LlmJudgeGuard`, `RegexRouter`, `OnnxRouter` (gated). `Orchestrator`
  constructor extended with optional `candidates=` and `router=`
  kwargs.
- New facade modules: `tako.routers` (`RegexRouter`, `OnnxRouter`),
  `tako.guards` (`RuleBased`, `LlmJudge`).
- New facade classes: `tako.Trinity`, `tako.SelfCaller`.
- New helper: `tako._native.featurise_text(text)` exposed for the
  Rust↔Python parity test.
- The `onnx` Cargo feature is forwarded from `tako-py` to
  `tako-orchestrator` so wheels can be built with or without the
  ort-loaded router.

### 3.I — Docs

- `docs/concepts/routing.md` — the `Router` trait, RegexRouter,
  OnnxRouter, OTel attributes, composition with SingleAgent.
- `docs/concepts/self_caller.md` — bounded recursion, depth tracking,
  guard impls, when (not) to use.
- `docs/recipes/trinity.md` — rule-based + trained ONNX walkthrough.
- `docs/recipes/self_caller.md` — RuleBased and LLM-judge variants;
  Trinity + SelfCaller composition.
- `docs/recipes/eval_harness.md` — synthetic dataset + custom JSONL +
  CLI usage.
- `docs/concepts/orchestrators.md` — extended with Trinity and
  SelfCaller sections (replacing the previous Phase-3 preview block).
- `mkdocs.yml` nav updated. `mkdocs build --strict` is clean.

### 3.J — Examples

- `examples/13_trinity_router.py` — Trinity over 3 Fake providers via
  RegexRouter.
- `examples/14_self_caller.py` — SelfCaller with `RuleBased` guard.
- `examples/15_eval_harness.py` — synthetic dataset → report.json.

## Verification (Definition of Done — Phase 3)

```bash
# Rust
cargo fmt --all -- --check                          # clean
cargo clippy --workspace --all-targets -- -D warnings   # clean
cargo test --workspace                              # 127 tests passing
                                                    #   (was 103 in v0.3.0)
cargo test -p tako-orchestrator --features onnx     # +1 ignored ONNX test

# Python
maturin develop --release
pytest -q tests/python                              # 66 tests passing (+19)
ruff check python/ tests/python/ examples/
mypy python/tako

# Wheel
maturin build --release
python -c "import tako; print(tako.__version__)"   # → 0.4.0

# Docs
mkdocs build --strict                               # clean

# Phase 3 acceptance gates (DoD per prompt.md §18 line 1035)
pytest tests/python/test_trinity.py::test_trinity_routes_three_providers
pytest tests/python/test_self_caller.py::test_terminates_within_max_depth_on_adversarial
pytest tests/python/test_eval.py::test_synthetic_runs_10_tasks_and_emits_report
```

## Acceptance checklist

- [x] Trinity router selects between 3 providers in tests
      (`test_trinity_routes_three_providers`)
- [x] SelfCaller terminates within `max_depth` on adversarial inputs
      (`test_terminates_within_max_depth_on_adversarial`)
- [x] Eval harness runs a 10-task synthetic benchmark and emits a JSON
      report (`test_synthetic_runs_10_tasks_and_emits_report` +
      `test_report_serialises_to_json`)
- [x] Orchestrator-native streaming (carry-over from Phase 2.5)
      replaces SingleAgent + Conductor stubs
- [x] Featuriser parity Rust ↔ Python
      (`tests/python/test_features_parity.py`)
- [x] CHANGELOG `## [0.4.0]` complete
- [x] mkdocs build --strict clean

## Scope decisions (confirmed with user, 2026-04-29)

- All four spec deliverables (Trinity rule + ONNX, training harness,
  SelfCaller, eval harness) plus the streaming carry-over shipped
  together in v0.4.0 (single release, ~17 commits per the original
  plan).
- ONNX is a Cargo feature `onnx` off by default. The wheel ships
  without ort/ndarray; users opt in via `maturin build --features onnx`.
- `SingleAgentBuilder` accepts an optional `Router` so per-step model
  selection is available without role-switching. Backwards-compatible.

## Phase 4 (next milestone)

- `AbMcts` orchestrator with pluggable `Verifier` trait + Thompson
  sampling, per arXiv:2503.04412 (TreeQuest).
- `Mistral`, `Ollama` providers.
- WebSocket and gRPC MCP transports.
- Sigstore signature verification for tool catalogues.
- Multi-tenant Redis-backed budget tracker.
- External eval datasets: SWE-Bench-Lite, GPQA-Diamond loaders.
- Trinity streaming (forward provider deltas through the routed turn).
