# PLAN — Phase 10 (Phase 9 follow-on completeness + cross-orchestrator verifier scores + Python provider streaming)

## Context

Phase 9 (v0.10.0, 2026-04-30) shipped:

- `LlmJudgeGuard::with_streaming_min_chars` / `with_streaming_every_n` opt-in streaming judge calls (9.A).
- `KeylessVerifier::with_rekor_min_tree_size` / `rekor_max_tree_size()` **in-memory** Rekor checkpoint freshness anchor (9.B). Persistence is hand-rolled by operators today.
- `event_to_tako_extensions` emitting named `tako.verifier_score` / `tako.recursion` SSE frames in `tako-compat` (9.C). All other `OrchEvent` variants return an empty `Vec`.
- AB-MCTS router-driven branch expansion via `AbMctsBuilder::candidate(p) / .router(r)` (9.D).
- README feature matrix swept current to Phases 7/8/9 (9.E).

[PLAN.md](/Users/kwc/tako-ai-core/PLAN.md) `## Roadmap → Phase 10 candidates` enumerates the natural follow-ons. Phase 10 picks four of them — the three small/medium items that round out Phase 9 cleanly, plus one Phase 2 stale marker — and defers the larger discrete items (http-generic streaming, vision content, eval-harness graders, MCP Streamable HTTP SSE upgrade) to dedicated future phases.

**Theme:** *follow-on completeness + cross-orchestrator parity + close one Phase 2 streaming gap.*

**Target tag:** v0.11.0.

## What this phase will land

### 10.0 — Plan-doc + version

- New per-phase plan doc: this file copied to `PLAN_PHASE10.md` (mirror of [PLAN_PHASE9.md](/Users/kwc/tako-ai-core/PLAN_PHASE9.md) structure).
- [PLAN.md](/Users/kwc/tako-ai-core/PLAN.md) phase-index table: add Phase 10 row, status `in progress`, then flip to `done (date)` at end of phase.
- Workspace package version: `0.10.0` → `0.11.0` in `Cargo.toml` (workspace + every per-crate `version =`), `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 10.A — On-disk `JsonStateStore` for Rekor freshness

Phase 9.B's `KeylessVerifier::with_rekor_min_tree_size(u64)` / `rekor_max_tree_size() -> u64` API was deliberately left forward-compatible with a persistence helper. Phase 10.A ships that helper.

**File to add:** `crates/tako-governance/src/sigstore_state.rs` (new module, exported from [crates/tako-governance/src/lib.rs](/Users/kwc/tako-ai-core/crates/tako-governance/src/lib.rs)).

```rust
/// On-disk JSON state for `KeylessVerifier::rekor_max_tree_size`.
///
/// Schema: `{ "rekor_min_tree_size": u64 }`. The file is written via
/// the standard `write-temp-then-rename` atomic pattern so a crash mid-
/// write cannot leave a corrupt anchor.
#[derive(Debug, Clone)]
pub struct JsonStateStore {
    path: PathBuf,
}

impl JsonStateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self;

    /// Read the persisted `rekor_min_tree_size`. Returns `Ok(0)` when
    /// the file does not exist (first-boot semantics).
    pub fn load(&self) -> Result<u64, TakoError>;

    /// Persist a new high-water mark. Writes to `<path>.tmp` then
    /// `rename` over `<path>` for crash-safety.
    pub fn save(&self, n: u64) -> Result<(), TakoError>;

    /// Convenience: load → seed verifier → return verifier.
    pub fn seed(&self, v: KeylessVerifier) -> Result<KeylessVerifier, TakoError>;

    /// Convenience: read `v.rekor_max_tree_size()` and persist it.
    pub fn persist(&self, v: &KeylessVerifier) -> Result<(), TakoError>;
}
```

**Integration:** `JsonStateStore::seed` calls `KeylessVerifier::with_rekor_min_tree_size`, `JsonStateStore::persist` reads `rekor_max_tree_size()`. Operators wrap `verify_bundle` with these two calls.

**PyO3:** new `tako._native.JsonStateStore` class with `__init__(path: str)`, `load() -> int`, `save(n: int) -> None`, `seed(v: KeylessVerifier) -> KeylessVerifier`, `persist(v: KeylessVerifier) -> None`. Forward through `tako.sigstore.JsonStateStore`. Stub in `_native.pyi`.

**Tests** in `crates/tako-governance/tests/sigstore_state.rs` (new file):
1. **Round-trip** — `save(7)` → `load() == 7`.
2. **First-boot** — `load()` against a non-existent path returns `Ok(0)` (does not error).
3. **Seed + persist cycle** — seed a fresh verifier from `5`, verify a `tree_size=8` bundle, call `persist`, re-load → `8`.
4. **Atomic write** — verify the `.tmp` file does not linger after a successful save.

Plus 1 Python smoke (`tests/python/test_phase10_state_store.py`).

**Public API risk:** purely additive new module + new Python facade class.

### 10.B — `tako-compat` named SSE events for tool-call lifecycle

Phase 9.C's `event_to_tako_extensions` ([crates/tako-compat/src/sse.rs:161-184](/Users/kwc/tako-ai-core/crates/tako-compat/src/sse.rs#L161-L184)) emits `tako.verifier_score` and `tako.recursion`; everything else falls through to `Vec::new()`. The Phase 9 PLAN explicitly called out tool-call lifecycle as the next natural extension. Today:

- `OrchEvent::ToolCallStart` IS already mapped to OpenAI's `tool_calls` delta in `event_to_payloads` ([sse.rs:92-114](/Users/kwc/tako-ai-core/crates/tako-compat/src/sse.rs#L92-L114)). Adding a `tako.tool_call_start` named event runs alongside it (zero impact: OpenAI clients ignore unknown `event:` lines).
- `OrchEvent::ToolCallResult` is **silently dropped** today ([sse.rs:136-139](/Users/kwc/tako-ai-core/crates/tako-compat/src/sse.rs#L136-L139)). This is the bigger gap — tako-aware clients have no way to observe tool results mid-stream. A `tako.tool_call_result` named event closes it.

**Change:** extend `event_to_tako_extensions` with two new arms:

```rust
OrchEvent::ToolCallStart { step, name, id } => {
    let body = json!({ "step": step, "name": name, "id": id });
    vec![("tako.tool_call_start", body.to_string())]
}
OrchEvent::ToolCallResult { step, id, result, is_error } => {
    let body = json!({
        "step": step, "id": id, "result": result, "is_error": is_error,
    });
    vec![("tako.tool_call_result", body.to_string())]
}
```

The route stream builder ([crates/tako-compat/src/routes.rs:145-186](/Users/kwc/tako-ai-core/crates/tako-compat/src/routes.rs#L145-L186)) already emits everything `event_to_tako_extensions` returns; no plumbing change.

**Tests** (`crates/tako-compat/src/sse.rs::tests` + `crates/tako-compat/tests/server.rs`):

- 2 new unit tests:
  - `ToolCallStart { step: 1, name: "search", id: "tc-1" }` → exactly one entry, name `tako.tool_call_start`, payload deserialises to `{"step":1,"name":"search","id":"tc-1"}`.
  - `ToolCallResult { step: 1, id: "tc-1", result: json!({"ok": true}), is_error: false }` → exactly one entry, name `tako.tool_call_result`.
- 1 new integration test `stream_emits_tool_call_lifecycle_extensions` in `tests/server.rs`: scripted orchestrator yields `ToolCallStart` then `ToolCallResult`; assert wire body contains both `event: tako.tool_call_start` and `event: tako.tool_call_result` framed before/around the OpenAI `tool_calls` delta.

The OpenAI SDK conformance test continues to pass (named events are silently ignored per the SSE spec).

**Public API risk:** additive on the wire (new `event:` lines that old clients ignore). No new Rust function — extends the existing `event_to_tako_extensions`.

### 10.C — `OrchEvent::VerifierScore` for `Trinity` and `Conductor`

`OrchEvent::VerifierScore { step, branch, score }` has been on the wire since v0.9.0 but only [`AbMcts`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs) emits it. PLAN.md's Phase 10 candidate #2 calls for parity in `Trinity` and `Conductor`. Phase 10.C adds an **optional** verifier to both — `None` keeps current behaviour byte-for-byte.

**`Trinity`:** verifier runs once per turn after the role's provider call resolves (the role itself often *is* a verifier, but an external `Verifier` adds an objective check). `branch` reuses the role's positional index in `role_order`.

**`Conductor`:** verifier runs once per worker output before fold-in. `branch` is the 1-based worker dispatch index within the current step.

**Builder additions** (mirror `AbMctsBuilder::verifier`):

```rust
// crates/tako-orchestrator/src/trinity.rs
impl TrinityBuilder {
    pub fn verifier(mut self, v: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(v);
        self
    }
}

// crates/tako-orchestrator/src/conductor.rs
impl ConductorBuilder {
    pub fn verifier(mut self, v: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(v);
        self
    }
}
```

Both structs gain `verifier: Option<Arc<dyn Verifier>>` as a new field.

**Emission point — Trinity:** in the `stream` body at [trinity.rs:540 / 657](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/trinity.rs) (after the role's `AssistantText` is fully buffered for the step, before yielding `Final`). When `verifier.is_some()`, call `v.score(&principal, &prompt, &output_text).await?`, `yield OrchEvent::VerifierScore { step, branch: role_position as u32, score }`.

**Emission point — Conductor:** in the worker fan-out loop (after each worker future resolves but before the result is folded into the next coordinator turn). `step` is the coordinator turn; `branch` is the 1-based dispatch index.

**Cost note:** verifier calls *can* be expensive (e.g. LLM-as-judge). Phase 10.C does not add streaming-aware variants — the verifier runs only at synthesis-complete boundaries, never per-delta. Per-delta judging remains the `LlmJudgeGuard` opt-in.

**PyO3:** `tako._native.Conductor.__init__` and `tako._native.Trinity.__init__` gain optional `verifier=` kwargs, forwarded through `tako.Conductor` / `tako.Trinity`. `_native.pyi` updated. `tako.verifier.Verifier` (the Python facade for `Arc<dyn Verifier>`, already in tree) is reused.

**Tests** (new `verifier_emits` sub-mods):

- `crates/tako-orchestrator/tests/trinity.rs::verifier_emits`:
  - **Emits when set** — Trinity with two roles + a `FixedScoreVerifier(0.6)`. Stream a single-turn run; assert exactly one `VerifierScore { step: 0, branch: <role_idx>, score: 0.6 }` is observed before `Final`.
  - **No-emit when unset** — same setup without `.verifier(...)`; no `VerifierScore` events.
- `crates/tako-orchestrator/tests/conductor.rs::verifier_emits`:
  - **Per-worker emit** — Conductor with a coordinator that dispatches three workers in one turn + a `FixedScoreVerifier(0.4)`. Stream the run; assert exactly three `VerifierScore` events with `branch` ∈ {1, 2, 3}.
  - **No-emit when unset** — same setup without `.verifier(...)`; no `VerifierScore` events.
- 1 Python smoke per orchestrator (`tests/python/test_phase10_conductor_verifier.py`, `tests/python/test_phase10_trinity_verifier.py`) using `FakeProvider` + a constant-score Python verifier.

**Public API risk:** additive (new builder method, new optional field, new Python kwarg). Without the kwarg, behaviour is byte-for-byte identical to v0.10.0.

### 10.D — Python custom provider streaming

[crates/tako-py/src/py_python_provider.rs:148-156](/Users/kwc/tako-ai-core/crates/tako-py/src/py_python_provider.rs#L148-L156) errors `"Python providers do not yet support streaming (Phase 2)"`. Phase 10.D closes this Phase 2 stale marker.

**Contract:** the Python callable becomes either a single `chat=` (current sync-text behaviour, unchanged) or accepts an optional second `stream=` callable that returns an `AsyncIterator[dict]`:

```python
async def stream(request: dict) -> AsyncIterator[dict]:
    yield {"kind": "delta", "text": "hello"}
    yield {"kind": "delta", "text": " world"}
    yield {"kind": "end", "finish_reason": "stop",
           "usage": {"input_tokens": 5, "output_tokens": 3}}
```

The `kind` discriminator matches the `tako-core` `ChatChunk` JSON tag (already `Delta` / `End`). Yielded dicts deserialise into `ChatChunk` via `serde_json::from_value`. Errors during parsing are wrapped as `TakoError::Provider`.

**Implementation sketch** (in [py_python_provider.rs:148](/Users/kwc/tako-ai-core/crates/tako-py/src/py_python_provider.rs#L148)):

```rust
async fn stream(
    &self,
    p: &Principal,
    r: ChatRequest,
) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
    let Some(stream_callable) = self.stream_callable.as_ref() else {
        return Err(TakoError::Invalid(format!(
            "python provider {} did not register a stream callable",
            self.id,
        )));
    };
    // 1. attach GIL, build request dict, call Python `stream(request)` → async iterator
    // 2. wrap the async iterator with `pyo3_async_runtimes::tokio::into_stream_v2`
    //    (returns a `Stream<Item = Py<PyAny>>` outside the GIL)
    // 3. map each `Py<PyAny>` to ChatChunk via with_gil + pythonize::depythonize
    // 4. terminate cleanly when the iterator raises StopAsyncIteration
    ...
}
```

GIL discipline mirrors the existing `chat()` impl: never hold the GIL across `.await`; use `Python::detach` / `with_gil` on each conversion boundary.

**Capability flag:** when a stream callable is registered, set `Capabilities::supports_streaming = true`. Today it's hardcoded `false` ([py_python_provider.rs:178](/Users/kwc/tako-ai-core/crates/tako-py/src/py_python_provider.rs#L178)).

**PyO3:** extend `PyPythonProvider::__init__` to accept an optional `stream=` kwarg:

```python
tako.providers.PythonProvider(
    id="my-llm",
    chat=async_chat_fn,
    stream=async_stream_fn,        # NEW; optional
    max_context_tokens=32_000,
)
```

The `tako.providers.PythonProvider` Python facade adds the same kwarg.

**Cancellation:** when the consumer drops the Rust stream, the underlying Python coroutine must be cancelled. `pyo3_async_runtimes::tokio` handles this for `into_stream_v2` (calls `aclose` on iterator drop). Add a regression test that drops the stream after the first chunk and asserts the Python `aclose` is invoked.

**Tests** (`crates/tako-py/tests/python_provider_streaming.rs` + `tests/python/test_phase10_python_streaming.py`):

- Rust side runs an inline Python provider via `pyo3` test harness; asserts streaming order: 2× `Delta`, 1× `End` with usage attached.
- Python smoke: `PythonProvider(id, chat, stream)` plugged into `SingleAgent`, calls `.stream(...)`, verifies received chunks and that the `streaming_judge` example shape works against a Python custom provider.

**Public API risk:** additive — current Python providers without `stream=` continue to work and continue to error on `.stream(...)` with the existing message (slightly reworded to "did not register a stream callable").

### 10.E — Examples + CHANGELOG + final flip

- New `examples/23_state_store.py` — `JsonStateStore` round-trip: seed `KeylessVerifier`, verify a bundle, persist.
- New `examples/24_tool_call_named_events.py` — minimal `tako-compat` server emitting `tako.tool_call_start` / `tako.tool_call_result` against a fake tool, consumed by a raw SSE client (no OpenAI SDK).
- New `examples/25_conductor_verifier.py` — `Conductor` with three workers + a `FixedScoreVerifier`; print `VerifierScore` events from the stream.
- New `examples/26_python_streaming_provider.py` — `tako.providers.PythonProvider(chat=..., stream=...)` plugged into `SingleAgent`, prints token-by-token output.
- `CHANGELOG.md` — new `## [0.11.0]` block summarising 10.A–10.D under `### Added` and `### Changed`. Compare-link appended at bottom.
- `README.md` feature matrix: append a Phase 10 column with checks for the four new features.
- `README.md` Roadmap section: append Phase 10 bullet.
- `PLAN.md` phase-index table: flip Phase 10 to `done (date)`. Update "Phase 10 candidates" → "Phase 11 candidates", carrying forward the deferred items: `http-generic` streaming, vision content, eval-harness graders, MCP Streamable HTTP SSE upgrade.

## Critical files

| File | Phase 10 part | Change |
|------|---------------|--------|
| `crates/tako-governance/src/sigstore_state.rs` (new) | 10.A | New module |
| `crates/tako-governance/src/lib.rs` | 10.A | Re-export |
| `crates/tako-governance/tests/sigstore_state.rs` (new) | 10.A | New tests |
| `crates/tako-py/src/py_state_store.rs` (new) | 10.A | PyO3 facade |
| `python/tako/sigstore.py` | 10.A | Forward `JsonStateStore` |
| `crates/tako-compat/src/sse.rs:161-184` | 10.B | Extend `event_to_tako_extensions` |
| `crates/tako-compat/tests/server.rs` | 10.B | Integration test |
| `crates/tako-orchestrator/src/trinity.rs` | 10.C | Add `verifier` field + emit |
| `crates/tako-orchestrator/src/conductor.rs` | 10.C | Add `verifier` field + emit |
| `crates/tako-orchestrator/tests/trinity.rs` | 10.C | New `verifier_emits` mod |
| `crates/tako-orchestrator/tests/conductor.rs` | 10.C | New `verifier_emits` mod |
| `crates/tako-py/src/py_trinity.rs`, `py_conductor.rs` | 10.C | `verifier=` kwarg |
| `crates/tako-py/src/py_python_provider.rs:148-156` | 10.D | Real streaming impl |
| `crates/tako-py/tests/python_provider_streaming.rs` (new) | 10.D | New tests |
| `python/tako/providers.py` | 10.D | `stream=` kwarg |
| `crates/tako-py/python/tako/_native.pyi` | 10.A/C/D | Stub updates |
| `Cargo.toml` (workspace + per-crate) | 10.0 | `0.10.0` → `0.11.0` |
| `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py` | 10.0 | Version bump |
| `examples/23_state_store.py` … `26_python_streaming_provider.py` | 10.E | New examples |
| `CHANGELOG.md` | 10.E | New `## [0.11.0]` block |
| `README.md` | 10.E | Feature matrix + Roadmap row |
| `PLAN.md` | 10.0 / 10.E | Index in/out |
| `PLAN_PHASE10.md` (new) | 10.0 | Per-phase plan |

## Reused utilities (avoid re-inventing)

- `KeylessVerifier::with_rekor_min_tree_size` / `rekor_max_tree_size` ([sigstore.rs:505-515](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L505-L515)) — 10.A wraps these, doesn't replace them.
- `event_to_tako_extensions` ([sse.rs:161](/Users/kwc/tako-ai-core/crates/tako-compat/src/sse.rs#L161)) — 10.B extends, doesn't fork.
- `AbMcts` verifier wiring ([ab_mcts.rs:482-506](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L482-L506)) — 10.C mirrors the `score()` call + `yield OrchEvent::VerifierScore` pattern verbatim.
- `Verifier` trait + `FixedScoreVerifier` test fixture (already in `tako-core` + `tako-orchestrator` test utils) — 10.C tests reuse them.
- `PyImpl::chat` GIL hand-off pattern ([py_python_provider.rs:54-146](/Users/kwc/tako-ai-core/crates/tako-py/src/py_python_provider.rs#L54-L146)) — 10.D mirrors steps 1–3 for the streaming variant.
- `pyo3_async_runtimes::tokio::into_stream_v2` for Python `AsyncIterator` → Rust `Stream` — 10.D's only new dep surface.
- `OpenAI streaming` SSE parser pattern (`crates/tako-providers/openai/src/stream.rs`) — reference for 10.D `ChatChunk` shapes (do not import; pattern only).

## Verification (Definition of Done)

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p tako-governance --features "sigstore sigstore-protobuf"   # 10.A
cargo test -p tako-compat                                                # 10.B
cargo test -p tako-orchestrator                                          # 10.C
cargo test -p tako-py                                                    # 10.D

# Python
maturin develop --release --features "sigstore sigstore-protobuf"
pytest -q tests/python                                                   # +5 smoke tests, all green
ruff check python/ tests/python/ examples/                               # clean
ruff format --check python/ tests/python/ examples/                      # clean
mypy python/tako                                                         # clean

python -c "import tako; print(tako.__version__)"                         # → 0.11.0

# Examples (smoke-run, none require network or real keys)
python examples/23_state_store.py
python examples/24_tool_call_named_events.py
python examples/25_conductor_verifier.py
python examples/26_python_streaming_provider.py
```

## Acceptance gates

- `JsonStateStore::save(7)` + `JsonStateStore::load() == 7` round-trips. `load()` against a missing path returns `Ok(0)`. After `seed → verify → persist`, the on-disk value matches `KeylessVerifier::rekor_max_tree_size()`.
- `event_to_tako_extensions(&OrchEvent::ToolCallStart {..})` returns one entry named `tako.tool_call_start` with the JSON-encoded fields. `ToolCallResult` ditto. All other variants still emit the same set as v0.10.0.
- `Trinity::builder().verifier(v).build()` and `Conductor::builder().verifier(v).build()` cause `VerifierScore` events on the stream; without `.verifier(...)` no such events appear.
- `tako.providers.PythonProvider(id=..., chat=..., stream=async_gen)` plugged into `SingleAgent.stream(...)` yields `ChatChunk::Delta` for each Python yield and exactly one `ChatChunk::End` with the reported usage; dropping the stream cancels the Python coroutine cleanly.
- `CHANGELOG.md` `## [0.11.0]` block added; version bumped to `0.11.0` in all four locations.
- `PLAN_PHASE10.md` written; `PLAN.md` index flipped to `Phase 10 — done (date)`; "Phase 11 candidates" carries forward the four deferred items.
- README feature matrix shows a Phase 10 column populated; README Roadmap enumerates a Phase 10 bullet.

## Out of scope (intentional, with rationale)

- **`http-generic` streaming.** Format is unknowable without operator config (OpenAI-compat SSE? NDJSON? custom binary?). Designing a `StreamConfig` enum + JSON-pointer-style delta extractor is a phase of its own. Defer to Phase 11.
- **Vision / image content support across providers.** Anthropic/Vertex/Bedrock all have stub markers; this is a multi-crate cross-cutting effort that warrants a focused phase.
- **Eval harness real graders (SWE-Bench Lite, GPQA Diamond).** Real SWE-Bench needs a sandboxed repo-test runner; standalone effort.
- **MCP Streamable HTTP — SSE upgrade + `Mcp-Session-Id` lifecycle.** Promised since Phase 2; protocol-spec implementation that warrants its own phase.
- **Streaming-aware verifier in Trinity/Conductor.** Per-step verifier emit is enough; per-delta verifier calls would need the same opt-in cost-control surface as `LlmJudgeGuard::with_streaming_min_chars`. No consumer asks for it yet.
- **Verifier `branch` semantics overhaul.** The Phase 8 enum keeps `branch: u32` for compatibility. For Conductor we use 1-based worker dispatch index; for Trinity the role's positional index. Both fit `u32`. A semantic-tagged variant would be a breaking change and isn't justified.
- **Persisted-state backends beyond JSON.** Redis/sqlite-backed `StateStore`s are easy follow-ups but `JsonStateStore` covers the operator hand-roll case targeted in 9.B's "out of scope" note.

## Phase 11 candidates (carry-forward)

Updated from PLAN.md's current list at end of Phase 10:

- `http-generic` provider streaming (OpenAI-compat SSE + NDJSON parsers, opt-in via `StreamConfig` enum on `HttpGenericConfig`).
- Vision / image content support across providers (Anthropic, Vertex, Bedrock).
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).
- MCP Streamable HTTP — SSE upgrade + `Mcp-Session-Id` lifecycle (Phase 2 promise).
- Redis-backed `StateStore` (sibling to `JsonStateStore` for multi-replica deployments).
- Sigstore security hardening (review-driven). Land H1 + H2 + M1–M4 from [SECURITY_PHASE10.md](SECURITY_PHASE10.md): race-free freshness-anchor advance (`compare_exchange_weak` or `Mutex<u64>`), `0600` state-file mode + docstring, unique tmp filenames, `deny_unknown_fields` + schema `version`, `basicConstraints: cA=TRUE` enforcement in `verify_chain`, tmp cleanup on rename failure. Strictly additive; no public API change.
