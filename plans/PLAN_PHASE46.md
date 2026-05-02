# PLAN — Phase 46 (Phase-1 placeholder sweep)

> **Status: in progress.** Targets v0.47.0. Closes three
> independent Phase-1 placeholders identified in the
> tech-debt review after Phase 45.

## Context

Phase 1 shipped foundational scaffolding marked as
"placeholder" / "deferred to a later phase" in the source.
Most have been resolved in subsequent phases, but three
small placeholders remain:

| # | Where | What | Why it matters |
|---|-------|------|----------------|
| 46.A | [python/tako/eval/harness.py:9-10](../python/tako/eval/harness.py#L9-L10) | Docstring claims `swe_bench_lite` and `gpqa_diamond` raise `NotImplementedError`. They don't — Phase 4 wired up real `load_*` functions. The docstring is actively misleading. | User-facing — anyone reading the module docstring is told the wrong thing. |
| 46.B | [python/tako/orchestrator.py:15-18](../python/tako/orchestrator.py#L15-L18) | `_Result` is a `text`-only placeholder. The comment says "Future versions will include usage, full message, and step count." The Rust `OrchOutput` carries `text`, `message`, `usage`, `steps` — but `tako-py` discards everything except `text`. | Operators have no programmatic access to token counts or step counts from Python. The Rust API is fully populated; the Python facade is throwing data away. |
| 46.C | [crates/tako-providers/vertex/src/convert.rs:373-383](../crates/tako-providers/vertex/src/convert.rs#L373-L383) | Vertex tool-call IDs are synthesised as `vertex_call_<n>` where `n` is the current `content` vector length. This is **not** stable across re-fetches of the same response or across streaming chunk boundaries — two chunks with different intervening content order produce different IDs for the same logical call. | Tool-result correlation breaks under streaming or retries. |

None of these are urgent enough to be a phase on their own;
they're small, orthogonal, low-risk cleanups. Bundling them
into one "placeholder sweep" phase matches the cadence of
Phase 34 (public-release prep / tech-debt + docs sweep).

## Why now

After Phase 45 (v0.46.0), the OIDC mTLS / private-CA story
is closed end-to-end. The remaining open backlog is
larger work (OTel real-collector e2e, eval-harness real
graders) that wants its own design phase. Sweeping the
small placeholder debt now keeps it from accumulating into
a worse problem.

The three items are independent — failure of any one
doesn't block the others — and each is small enough to land
in a single commit.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 46.A | Fix stale `harness.py` module docstring | [`python/tako/eval/harness.py`](../python/tako/eval/harness.py) |
| 46.B | Plumb `usage` + `steps` from Rust `OrchOutput` to Python `_Result` across all four orchestrators (SingleAgent, Conductor, SelfCaller, AbMcts, Trinity) | [`crates/tako-py/src/py_orchestrator.rs`](../crates/tako-py/src/py_orchestrator.rs), [`crates/tako-py/src/py_conductor.rs`](../crates/tako-py/src/py_conductor.rs), [`crates/tako-py/src/py_self_caller.rs`](../crates/tako-py/src/py_self_caller.rs), [`crates/tako-py/src/py_ab_mcts.rs`](../crates/tako-py/src/py_ab_mcts.rs), [`crates/tako-py/src/py_trinity.rs`](../crates/tako-py/src/py_trinity.rs), [`python/tako/orchestrator.py`](../python/tako/orchestrator.py) |
| 46.C | Replace Vertex placeholder ID with stable hash of `(name, args)` | [`crates/tako-providers/vertex/src/convert.rs`](../crates/tako-providers/vertex/src/convert.rs) |
| 46.D | Tests for all three changes | unit tests in same files; new Python smoke test for 46.B |
| 46.E | Workspace + Python version 0.46.0 → 0.47.0 | various |
| 46.F | PLAN.md row + close `Vertex deterministic-per-call placeholder logic` backlog item | [`PLAN.md`](../PLAN.md) |
| 46.G | CHANGELOG.md `[0.47.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 46.A — Stale `harness.py` docstring

Replace the misleading paragraph:

```python
"""...
Stub references for ``swe_bench_lite`` and ``gpqa_diamond`` exist for
forward compatibility but raise ``NotImplementedError`` — Phase 4 work.
"""
```

with:

```python
"""...
``swe_bench_lite`` and ``gpqa_diamond`` are loaded on-demand from
Hugging Face via :mod:`tako.eval.datasets.external` (requires
``pip install tako[eval]``). Verification is intentionally
lightweight: SWE-Bench uses substring-match on filenames in the gold
patch; GPQA uses an A/B/C/D positional verifier. Real SWE-Bench
grading (apply patch + run sandboxed repo tests) is deferred to a
later phase.
"""
```

This matches reality and points readers at the right module.

### 46.B — `_Result` plumbing

**Rust side** (one new pyclass shared across all orchestrators):

Add `PyOrchOutput` to `tako-py` (new module
[`crates/tako-py/src/py_orch_output.rs`](../crates/tako-py/src/py_orch_output.rs)):

```rust
#[pyclass(name = "OrchOutput", module = "tako._native", frozen)]
pub struct PyOrchOutput {
    inner: tako_orchestrator::OrchOutput,
}

#[pymethods]
impl PyOrchOutput {
    #[getter] fn text(&self) -> &str { &self.inner.text }
    #[getter] fn input_tokens(&self) -> u32 { self.inner.usage.input_tokens }
    #[getter] fn output_tokens(&self) -> u32 { self.inner.usage.output_tokens }
    #[getter] fn total_tokens(&self) -> u32 { self.inner.usage.total() }
    #[getter] fn steps(&self) -> u32 { self.inner.steps }
    fn __repr__(&self) -> String {
        format!(
            "OrchOutput(text={!r}, input_tokens={}, output_tokens={}, steps={})",
            // truncate text in repr
            ...,
            self.inner.usage.input_tokens,
            self.inner.usage.output_tokens,
            self.inner.steps,
        )
    }
}
```

`message` field stays internal for now — exposing it cleanly
needs Pydantic-side `ContentPart` round-tripping which is a
larger surface than the placeholder doc claimed. Defer until
operator ask.

**All five orchestrator pyclasses** (`PyOrchestrator`,
`PyConductor`, `PySelfCaller`, `PyAbMcts`, `PyTrinity`) update
their `run` / `run_sync` to return `PyOrchOutput` instead of
`String`:

```rust
fn run<'py>(...) -> PyResult<Bound<'py, PyAny>> {
    let agent = Arc::clone(&self.inner);
    let principal = ...;
    future_into_py(py, async move {
        let out = agent.run(&principal, OrchInput::from_user(prompt))
            .await
            .map_err(...)?;
        Ok(PyOrchOutput::from(out))
    })
}
```

**Python side** ([`python/tako/orchestrator.py`](../python/tako/orchestrator.py)):

Replace the `_Result(text)` placeholder. The new
`_Result` is a thin Pydantic-style dataclass that holds the
PyO3 `OrchOutput` instance and exposes `text`, `usage` (a
Pydantic `Usage`), and `steps` — `result.text` keeps
working unchanged (zero-cost for existing callers):

```python
class _Result:
    __slots__ = ("_inner",)
    def __init__(self, inner: Any) -> None:
        self._inner = inner
    @property
    def text(self) -> str: return self._inner.text
    @property
    def steps(self) -> int: return self._inner.steps
    @property
    def usage(self) -> Usage:
        return Usage(
            input_tokens=self._inner.input_tokens,
            output_tokens=self._inner.output_tokens,
        )
    def __repr__(self) -> str: return repr(self._inner)
```

All five orchestrators' `run` / `run_sync` methods change one line:

```diff
- text = await self._inner.run(...)
- return _Result(text)
+ out = await self._inner.run(...)
+ return _Result(out)
```

### 46.C — Vertex placeholder ID

Replace position-derived IDs with a deterministic hash of
the function call's `(name, args)` JSON:

```rust
use std::hash::{DefaultHasher, Hash, Hasher};

if let Some(fc) = part.function_call {
    had_tool_call = true;
    // Stable per-call ID: hash of (name, args) as JSON. Stable
    // across streaming chunk boundaries and re-fetches of the
    // same response — replaces the position-derived
    // `vertex_call_<n>` placeholder.
    let mut hasher = DefaultHasher::new();
    fc.name.hash(&mut hasher);
    serde_json::to_string(&fc.args).unwrap_or_default().hash(&mut hasher);
    let id = format!("vertex_call_{:016x}", hasher.finish());
    content.push(ContentPart::ToolCall {
        id,
        name: fc.name,
        args: fc.args,
    });
}
```

`DefaultHasher` is `SipHash13`; output is non-cryptographic
but deterministic per-process for a given input. The id
format keeps the `vertex_call_` prefix so log-grep
patterns continue to work; only the suffix changes from a
small integer to a hex digest.

(If two distinct calls in the same response happen to have
identical name + args JSON, they get the same ID — but
identical-name-and-args is exactly the case where
deduplication is desirable, not a bug.)

### 46.D — Tests

- **46.A**: no test (docstring change).
- **46.B**: new
  [`tests/python/test_phase46_orch_output_fields.py`](../tests/python/test_phase46_orch_output_fields.py)
  covering `result.text`, `result.usage.input_tokens`,
  `result.usage.output_tokens`, `result.usage.total`,
  `result.steps` for `SingleAgent` and `Conductor` (using
  `FakeProvider`). Existing tests using `result.text` keep
  passing unchanged.
- **46.C**: new unit test in
  [`crates/tako-providers/vertex/src/convert.rs`](../crates/tako-providers/vertex/src/convert.rs)
  asserting that two parses of the same response payload
  produce identical IDs (deterministic), and that distinct
  `(name, args)` produce distinct IDs.

### 46.E — Version bump

0.46.0 → 0.47.0 across `Cargo.toml` (workspace + 14 internal
crate version pins), `pyproject.toml`,
`python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 46.F — PLAN.md

- New row `46 — Phase-1 placeholder sweep`.
- Flip `Vertex deterministic-per-call placeholder logic`
  backlog item to closed-by-Phase-46.

### 46.G — CHANGELOG `[0.47.0]`

Standard format, noting all three sweeps + the new
`PyOrchOutput` pyclass.

## Critical files

**Modified:**
- [`python/tako/eval/harness.py`](../python/tako/eval/harness.py) — docstring (46.A).
- [`crates/tako-py/src/lib.rs`](../crates/tako-py/src/lib.rs) — register `PyOrchOutput` (46.B).
- [`crates/tako-py/src/py_orchestrator.rs`](../crates/tako-py/src/py_orchestrator.rs) (46.B).
- [`crates/tako-py/src/py_conductor.rs`](../crates/tako-py/src/py_conductor.rs) (46.B).
- [`crates/tako-py/src/py_self_caller.rs`](../crates/tako-py/src/py_self_caller.rs) (46.B).
- [`crates/tako-py/src/py_ab_mcts.rs`](../crates/tako-py/src/py_ab_mcts.rs) (46.B).
- [`crates/tako-py/src/py_trinity.rs`](../crates/tako-py/src/py_trinity.rs) (46.B).
- [`python/tako/orchestrator.py`](../python/tako/orchestrator.py) (46.B).
- [`crates/tako-providers/vertex/src/convert.rs`](../crates/tako-providers/vertex/src/convert.rs) (46.C).
- Standard PLAN/CHANGELOG/version flip.

**Created:**
- [`crates/tako-py/src/py_orch_output.rs`](../crates/tako-py/src/py_orch_output.rs) (46.B).
- [`tests/python/test_phase46_orch_output_fields.py`](../tests/python/test_phase46_orch_output_fields.py) (46.D).
- [`plans/PLAN_PHASE46.md`](PLAN_PHASE46.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy -p tako-py --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider" -- -D warnings`.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
4. `cargo test -p tako-providers-vertex` — new ID-stability test passes.
5. `cargo test --workspace --exclude tako-py --all-features` — no regressions.
6. `ruff format --check` + `ruff check`.
7. `maturin develop --release` — wheel builds at v0.47.0.
8. `pytest -q` — full suite + new test pass.

## Out of scope

- **Exposing `OrchOutput.message` from Python.** Requires
  Pydantic round-tripping of `ContentPart`/`Message` from
  Rust JSON → Python; larger surface than the
  placeholder-comment claim. Defer until operator ask.
- **Real SWE-Bench / GPQA grading.** Separate Phase 47+
  candidate (sandboxed container infra).
- **OTel real-collector e2e test.** Separate phase
  candidate.
- **Renaming `_Result` to `OrchResult`** as a public name.
  The leading underscore signals "private" and the docstring
  promises stability of `result.text` only — keeping the
  name avoids any compat concern.
