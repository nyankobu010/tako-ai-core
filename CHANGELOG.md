# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

(none)

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

[Unreleased]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/TODO(<org>)/tako-ai-core/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/TODO(<org>)/tako-ai-core/releases/tag/v0.1.0
