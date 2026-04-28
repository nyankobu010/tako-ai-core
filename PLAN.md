# PLAN — Phase 1: Foundation

> Per spec §19 rule 1: this is the rolling project plan that future Claude
> Code sessions read on entry. Update it as phases land or scope shifts.

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

| # | Commit | Status |
|---|--------|--------|
| 1 | License & governance docs | done |
| 2 | README + ARCHITECTURE + CLAUDE.md + PLAN.md | in progress |
| 3 | Workspace `Cargo.toml` + `rust-toolchain.toml` | pending |
| 4 | `tako-core` types + errors | pending |
| 5 | `tako-core` traits | pending |
| 6 | `tako-core` tests | pending |
| 7 | `tako-runtime` impl | pending |
| 8 | `tako-runtime` tests | pending |
| 9 | `tako-providers/openai` impl | pending |
| 10 | `tako-providers/openai` tests | pending |
| 11 | `tako-providers/anthropic` impl | pending |
| 12 | `tako-providers/anthropic` tests | pending |
| 13 | `tako-providers/http-generic` | pending |
| 14 | `tako-mcp` trait + stdio | pending |
| 15 | `tako-mcp` Streamable HTTP + registry | pending |
| 16 | `tako-mcp` tests | pending |
| 17 | `tako-orchestrator` `SingleAgent` + tests | pending |
| 18 | `tako-governance` OTel + PII + EnvResolver | pending |
| 19 | `pyproject.toml` + `tako-py` skeleton | pending |
| 20 | `tako-py` PyClient + PyOrchestrator | pending |
| 21 | `tako-py` provider bindings | pending |
| 22 | `tako-py` MCP / OTel / Budget bindings | pending |
| 23 | `python/tako/` facade + stubs | pending |
| 24 | examples 01, 06, 08 | pending |
| 25 | `tests/python/` smoke + concurrency | pending |
| 26 | `.github/` CI + dependabot + ISSUE_TEMPLATE | pending |
| 27 | `.github/workflows/wheels.yml` | pending |
| 28 | `docs/` mkdocs skeleton | pending |
