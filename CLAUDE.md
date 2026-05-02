# tako — guidance for Claude Code

See @README.md for project overview, @ARCHITECTURE.md for the design,
and @CONTRIBUTING.md for the dev workflow.

## What this project is
`tako` is a Rust-core, Python-facade framework for enterprise agentic systems.
The Rust workspace lives in `crates/`, the Python facade in `python/tako/`,
and the wheel target is `crates/tako-py` built with maturin + PyO3.

## Build & test
- `maturin develop --release` rebuilds the extension into the active venv.
- `cargo test --workspace` runs all Rust unit + integration tests.
- `pytest -q` runs the Python suite (uses an in-process `FakeProvider`; no API keys needed).
- `cargo clippy --workspace -- -D warnings` MUST pass before commit.
- `cargo fmt --all` and `ruff format python/` before commit.

## Code style
- Rust 2024 edition, MSRV 1.91.1, no `unsafe` outside `tako-py` FFI shims.
- All public traits use `#[async_trait]` and require `Send + Sync + 'static` impls.
- All errors use `thiserror`; never `unwrap()` or `expect()` in library code.
- Python uses Pydantic v2 models, `from __future__ import annotations`, and full type hints.
- Public Rust APIs have rustdoc with at least one runnable example.

## Workflow rules
- Make a `PLAN.md` before writing code for any new phase.
- Commit per logical unit (one crate, one feature, one fix); never bundle.
- After modifying a Rust crate, run `cargo test -p <crate>` before moving on.
- After modifying Python, run `pytest tests/python/<file>` before moving on.
- Keep `CHANGELOG.md` updated under the `## [Unreleased]` heading.
- Never commit secrets. `.env` is gitignored. Tests use `FakeProvider`.

## Architectural rules
- `tako-core` is dependency-light (no I/O, no Tokio). It defines traits and types only.
- Provider crates ONLY depend on `tako-core` + their vendor SDK; never on each other.
- `tako-py` is a thin binding layer; orchestration logic lives in `tako-orchestrator`.
- The Python facade in `python/tako/` is allowed to import `tako._native` only;
  end users import `tako.*`.

## Things NOT to do
- Do not add new top-level crates without updating `ARCHITECTURE.md`.
- Do not add provider-specific concepts to `tako-core` (e.g. "Anthropic-style content blocks").
- Do not introduce blocking I/O on the Tokio reactor; use `spawn_blocking` or async equivalents.
- Do not hold the Python GIL across `.await`; always use `Python::detach` (PyO3 0.28+ name)
  before awaiting.

## Phase discipline
Phase 1 ships the foundation (traits, runtime, two providers, MCP basics,
`SingleAgent`, OTel, PyO3 wheel, CI). Do **not** add Phase 2+ features
(Conductor, OPA, Trinity, AB-MCTS, Bedrock/Vertex, Sigstore, etc.) until
Phase 1 is green on all CI targets.
