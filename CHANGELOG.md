# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

(none)

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

[Unreleased]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/TODO(<org>)/tako-ai-core/releases/tag/v0.1.0
