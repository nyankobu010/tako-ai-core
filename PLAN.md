# PLAN — rolling project plan

> Per spec §19 rule 1: this is the rolling project plan that future Claude
> Code sessions read on entry. Update it as phases land or scope shifts.
>
> **Done**: Phase 1 (foundation, v0.1.0) + Phase 2 with bundled
> Phase 1.5 (orchestration, v0.2.0). See [PLAN_PHASE2.md](PLAN_PHASE2.md)
> and [CHANGELOG.md](CHANGELOG.md).
>
> **Next**: Phase 2.5 — Azure OpenAI + Vertex providers, Vault / cloud
> secret resolvers, full mkdocs nav. Or skip ahead to Phase 3 (Trinity
> learned routing). Plan when starting.

## Phase 1: Foundation

## Goal

Ship a working `pip install tako` that exposes a `SingleAgent` orchestrator
backed by Anthropic + OpenAI providers, with MCP stdio + Streamable HTTP
support, OTel tracing, in-memory budgets, circuit breakers, and a sync+async
Python API. Phase 1 is done when CI is green on Linux/macOS/Windows and a
clean-venv `pip install tako-*.whl && python -c "import tako; print(tako.__version__)"`
works.

## Scope (in)

- `tako-core` — five traits, types, errors. No I/O, no Tokio.
- `tako-runtime` — Tokio glue: budget, breaker, retry, limiter, fallback,
  Principal propagation.
- `tako-providers/anthropic` — Messages API, streaming SSE, tool calls.
- `tako-providers/openai` — chat.completions, streaming, tool calls.
- `tako-providers/http-generic` — template-driven generic HTTP provider.
- `tako-mcp` — `McpTransport` trait + stdio + Streamable HTTP via `rmcp`.
- `tako-orchestrator` — `SingleAgent` only.
- `tako-governance` — OTel pipeline, PII regex transform, `EnvResolver`.
- `tako-py` — PyO3 bindings; shared Tokio runtime; GIL discipline.
- `python/tako/` — Pydantic-v2 facade, `_native.pyi` stubs, `py.typed`.
- CI: fmt + clippy + cargo test + maturin + pytest + audits on 3 OSes.
- Docs: README quickstart, `ARCHITECTURE.md`, one mkdocs page.

## Scope (out — explicitly deferred)

| Item | Phase |
|------|-------|
| `Conductor`, OPA enforcement, OpenAI-compat server | 2 |
| Bedrock, Vertex, Azure-OpenAI providers | 2 |
| Vault / AWS SM / Azure KV / GCP SM secret resolvers | 2 |
| `Trinity` learned router (rule + ONNX), training harness | 3 |
| `SelfCaller` bounded recursion | 3 |
| Eval harness | 3 |
| `AbMcts` orchestrator | 4 |
| Mistral, Ollama providers | 4 |
| WebSocket + gRPC MCP transports | 4 |
| Sigstore tool-catalogue verification | 4 |
| Redis-backed `BudgetBackend` | 4 |

## Decisions (locked in 2026-04-28)

1. Project root is the repo root (`tako-ai-core/`); the package is `tako`.
2. `<org>` in URLs is the literal string `TODO(<org>)` — grep-able single point of edit at first publish.
3. Toolchain prerequisites are the contributor's responsibility; documented in `CONTRIBUTING.md`.
4. Crate versions pinned to current stable as of 2026-04-28 (see `CHANGELOG.md`).

## Verification (Definition of Done — Phase 1)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features -- -D warnings
cargo test --workspace --all-features
cargo audit
maturin develop --release
pytest -q tests/python
ruff check python/  &&  ruff format --check python/
mypy python/tako
maturin build --release
pip install target/wheels/tako-*.whl
python -c "import tako; print(tako.__version__)"   # → 0.1.0
```

CI replicates the above on Linux + macOS + Windows. Spec §22 final checklist
must be fully ticked.

## Status

Phase 1 has landed. As of 2026-04-28:

- `cargo fmt --all -- --check` — green
- `cargo clippy --workspace --all-targets -- -D warnings` — green
- `cargo test --workspace` — 53 tests + 4 doctests, all green

The Rust workspace ships nine crates (tako-core, tako-runtime, three
providers, tako-mcp, tako-orchestrator, tako-governance, tako-py).
The Python facade exposes `tako.providers.{OpenAI, Anthropic, Fake}`,
`tako.SingleAgent` (sync + async), `tako.Budget`, `tako.tracing.init`.
Tests live in `tests/python/` (smoke + async-concurrency).
CI on Linux/macOS/Windows + multi-target wheel build are configured.

### Phase 1.5 follow-ups (deferred from spec §13/§17)

These were documented in commits but did not land in Phase 1:

- PyO3 bindings for MCP transports (`tako.mcp.Stdio` / `tako.mcp.Http`)
  — Rust today, Python facade wraps them next.
- `PyPythonProvider` shim so users can implement custom providers
  in pure Python.
- OTLP exporter wiring (Phase 2 per spec, but the skeleton is here).
- mkdocs full nav (concepts/, recipes/, API reference) — Phase 2.

### Final Phase 1 verification (manual, requires Python 3.10+)

```bash
uv venv .venv && source .venv/bin/activate
uv pip install maturin pytest pytest-asyncio ruff mypy
maturin develop --release
pytest -q tests/python
ruff check python/ tests/python/ examples/
mypy python/tako
maturin build --release
pip install target/wheels/tako-*.whl
python -c "import tako; print(tako.__version__)"   # → 0.1.0
```

CI replicates this on Linux + macOS + Windows × Python 3.10–3.13.
