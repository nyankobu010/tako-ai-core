# PLAN — Phase 2 (with bundled Phase 1.5)

> **Status: complete (v0.2.0, 2026-04-29).** All four MUST-LAND
> deliverables landed plus the three bundled Phase 1.5 follow-ups.
> See [CHANGELOG.md](CHANGELOG.md) `## [0.2.0]` for the full list.
>
> Successor to [PLAN.md](PLAN.md). Everything from spec §18 Phase 2
> not covered here is deferred to Phase 2.5.

## Context

Phase 1 shipped a working Rust workspace + Python facade with a
SingleAgent orchestrator backed by Anthropic + OpenAI providers.
Several spec items were deferred from Phase 1 because they were either
not on the Phase-1 critical path (custom Python providers, Python MCP
bindings) or required upstream API research the time budget couldn't
absorb (OTLP exporter wiring against `opentelemetry-otlp` 0.31).
Those are now bundled into Phase 2 as **Phase 1.5**, ahead of the
Phase-2-proper features.

Phase 2's spec scope was wide (3 cloud providers + Conductor + OPA +
compat server + 4 secret resolvers + full docs site). The user picked
**Conductor + Bedrock + OPA + compat server** as the MUST-LAND scope;
**Azure OpenAI / Vertex / Vault / cloud secret resolvers / full
mkdocs nav** all slip to Phase 2.5.

## Decisions locked in (2026-04-28)

1. **Phase 1 close**: run `maturin develop --release && pytest -q tests/python && maturin build --release` as Step 0 of this phase before any new code lands. If the GIL-discipline test fails, fixing Phase 1 takes priority over Phase 2 features.
2. **Phase 1.5 ordering**: ship Phase 1.5 items first as a coherent block, then Phase 2 features. Avoids a long feature-incomplete window for Python users.
3. **Phase 2 scope (MUST-LAND)**: Conductor + 1 cloud provider (Bedrock) + OPA enforcement at all three policy stages + tako-compat OpenAI-compatible HTTP server.
4. **Phase 2.5 scope (deferred)**: Azure OpenAI provider, Vertex provider, Vault / AWS SM / Azure KV / GCP SM secret resolvers, full mkdocs nav (concepts + recipes + API reference) and GitHub Pages deploy.

## Phase 1.5 deliverables

### 1.5a — PyO3 MCP bindings
- `tako._native.Stdio(command, args)` wraps `tako_mcp::StdioTransport::spawn`
- `tako._native.Http(url, headers={}, timeout_secs=)` wraps `tako_mcp::StreamableHttpBuilder`
- New helper: `tako._native.discover_mcp_tools(transport) -> int` runs lifecycle handshake + `tools/list`, registers schemas back into the orchestrator
- Python facade: `tako.mcp.Stdio` / `tako.mcp.Http` replace today's NotImplementedError placeholders
- Wiring: `tako.SingleAgent(provider=..., mcp_servers=[...])` accepts MCP transports and discovers their tools at construction

### 1.5b — `PyPythonProvider` shim
Lets users implement `LlmProvider` in pure Python:

```python
class MyProvider(tako.providers.PythonProvider):
    id = "my:provider"
    capabilities = tako.Capabilities(...)
    async def chat(self, principal, req): ...
    async def stream(self, principal, req): ...
```

Implementation hinge: the Rust side stores a `Py<PyAny>` and impls
`LlmProvider` by acquiring the GIL to call the Python method, then
releases it via `pyo3_async_runtimes::tokio::into_future` while
awaiting the resulting Python coroutine.

### 1.5c — OTLP exporter actual wiring
Replace the warn-and-delegate stub in `tako-governance::otel`:
- `init_otlp_tracing(endpoint, protocol="grpc"|"http", resource_attrs)` builds a real `opentelemetry-otlp` exporter with `BatchSpanProcessor`, attaches it to `tracing-opentelemetry::layer()`, returns a `TracerGuard` that flushes on drop.
- Default resource attributes: `service.name = "tako"`, `service.version = env!("CARGO_PKG_VERSION")`.
- Smoke test: in-process collector receives a `tako.orchestrator.run` span with the documented `tako.*` + `gen_ai.*` attributes.

### 1.5d — Async-concurrency CI lock-in
The Phase-1 `test_async_concurrency.py` runs locally now that Python is wired up; ensure CI on Linux/macOS/Windows × 3.10–3.13 actually executes it (not just compiles).

## Phase 2 deliverables

### 2.A — Bedrock provider
- New crate `crates/tako-providers/bedrock/`
- Uses `aws-config` 1.8 for default credential chain (env, profile, IRSA, IMDS) + `aws-sdk-bedrockruntime` 1.130's Converse API
- `BedrockBuilder::region(...)`, `model("anthropic.claude-3-5-sonnet-20240620-v1:0")`, `profile(...)`, `endpoint_url(...)` for VPC endpoints
- Provider id: `"bedrock:<model_id>"`
- Streaming via Bedrock's `ConverseStream` API → `ChatChunk` adapter (similar shape to Anthropic SSE; reuses `ContentPart::ToolCall` / `ToolCallDelta` types)
- Wiremock-backed integration tests for the Converse JSON shape (the AWS SDK is replaceable with `endpoint_url` for tests)
- Workspace deps add: `aws-config`, `aws-sdk-bedrockruntime`

### 2.B — Conductor orchestrator
- New module `crates/tako-orchestrator/src/conductor.rs`
- Implements `Orchestrator` trait, kind `OrchestratorKind::Conductor`
- Coordinator LLM emits **structured dispatch JSON** (a strict schema we validate; coordinator gets the schema in its system prompt):
  ```json
  {
    "workers": [
      {"name": "code", "task": "implement X", "tools": ["fs", "git"]},
      {"name": "math", "task": "verify the bound"}
    ],
    "join_strategy": "all" | "any",
    "next_step": "summarise" | "halt"
  }
  ```
- Worker pool: `HashMap<String, Arc<dyn LlmProvider>>` keyed by role name; dispatched concurrently via `tokio::spawn` + `Arc<Semaphore>` for `max_fanout`
- Knobs: `max_steps`, `max_fanout`, `worker_timeout`, `fail_fast`
- Each worker dispatch is its own `tako.orchestrator.dispatch` span with `worker.name`, `worker.provider.id`
- Tests: 1) coordinator + 2 workers (FakeProvider scripts) finishes; 2) `max_fanout` semaphore limits concurrency; 3) `fail_fast: true` aborts on first worker error; 4) `fail_fast: false` collects partial results.

### 2.C — OPA/regorus integration
- `tako-governance::policy` module wraps `regorus::Engine`
- `OpaBundle::from_path(...)` / `OpaBundle::from_string(...)`; compiled engines cached per bundle SHA-256
- Three enforcement entry points (already typed as `PolicyStage::PreChat / PreTool / PostChat` in `tako-core`):
  - `pre_chat`: `{principal, model, messages_hash, tools}` → `Allow` / `Deny` / `RedactMessages` / `ForceModel`
  - `pre_tool`: `{principal, tool, args_hash}` → `Allow` / `Deny` / `RequireApproval`
  - `post_chat`: `{principal, response_hash}` → `Allow` / `RedactResponse`
- `SingleAgent` and `Conductor` both consult the engine before each provider call and before each tool call; deny → `TakoError::PolicyDenied` propagates.
- Audit log: append-only JSONL writer (`AuditLog::jsonl(path)`) records every decision with timestamp, principal, stage, decision. Phase 4's SIEM exporter reuses this format.
- Default policy in `examples/policies/`: an "allow all but log everything" Rego file.
- Test: a Rego rule denies tool name `"shell.exec"` for tenants without `roles: ["admin"]`; orchestrator surfaces `PolicyDenied` and audit log records the event.

### 2.D — `tako-compat` OpenAI-compatible server
- New crate already scaffolded but empty; flesh it out with `axum` 0.8.
- Routes:
  - `POST /v1/chat/completions` (streaming SSE + non-streaming; OpenAI tool-call delta format on the wire)
  - `GET  /v1/models` — surfaces every provider's id + capabilities
  - `GET  /healthz`, `GET  /readyz`
- Auth: bearer-token middleware → `AuthResolver` trait (default impl: static token map). The resolved `Principal` flows into `SingleAgent`/`Conductor`.
- Per-tenant budget + policy lookup via `Principal.tenant_id`.
- Streaming: maps internal `ChatChunk::Delta` → OpenAI streaming delta JSON; emits `data: [DONE]` terminator.
- Test: boots server on `127.0.0.1:0` in a background task and uses the official `openai` Python SDK pointed at the local URL to verify the wire format end-to-end.

## File map (Phase 2)

```
crates/
├── tako-providers/
│   └── bedrock/                 ← NEW (2.A)
│       ├── Cargo.toml
│       ├── src/{lib, client, convert, stream}.rs
│       └── tests/chat.rs
├── tako-orchestrator/
│   └── src/conductor.rs         ← NEW (2.B)
├── tako-governance/
│   └── src/{policy, audit}.rs   ← NEW (2.C)
├── tako-compat/
│   ├── src/{lib, server, routes/chat, routes/models, auth, sse}.rs   ← NEW (2.D)
│   └── tests/openai_sdk.rs
└── tako-py/
    └── src/{py_mcp, py_python_provider, py_otel}.rs                  ← NEW (1.5a/b/c)

python/tako/
├── mcp.py            ← rewrite: real Stdio/Http (1.5a)
├── providers.py      ← add PythonProvider (1.5b)
└── tracing.py        ← add real Otlp(...) (1.5c)

examples/
├── 02_conductor.py          ← NEW
├── 06_mcp_stdio.py          ← NEW (uses 1.5a bindings)
├── 07_openai_compat_server.py ← NEW (uses 2.D)
└── policies/allow_with_audit.rego  ← NEW (2.C example bundle)

tests/
├── rust/conductor.rs        ← NEW (2.B end-to-end)
└── python/
    ├── test_mcp_stdio.py    ← NEW (1.5a)
    ├── test_python_provider.py  ← NEW (1.5b)
    ├── test_compat_server.py    ← NEW (2.D against openai SDK)
    └── test_otlp.py             ← NEW (1.5c with stub collector)

CHANGELOG.md         ← updated under [Unreleased] for every commit
PLAN.md              ← rolling status table (this file lives alongside)
```

## Critical implementation notes

These lock decisions in now so they don't get re-litigated mid-execution:

1. **Bedrock streaming uses Bedrock's protobuf event-stream framing**, not SSE. Don't reach for `eventsource-stream` here — `aws-sdk-bedrockruntime` already returns a `ConverseStreamOutput` async stream. The adapter just maps each Bedrock event to `ChatChunk`.

2. **Conductor's coordinator output is JSON-validated, not free-form**. We give the coordinator the JSON schema in its system prompt, parse with `serde_json::from_str` against a strongly-typed `DispatchPlan` struct. Reject malformed plans with a synthetic ChatResponse asking for retry, capped at `max_steps`.

3. **OPA bundle hash caching**. `regorus::Engine::add_policy` is expensive; cache `Arc<Engine>` per bundle SHA-256 in a `Mutex<HashMap<[u8; 32], Arc<Engine>>>`. Recompile only when the bundle path's mtime changes (filesystem) or the in-memory string is replaced.

4. **`tako-compat` does NOT pull in the AWS SDK**. The compat server depends on `tako-orchestrator` + `tako-governance` + `tako-runtime`. Cloud providers are pulled by the *binary* user, not by the compat-server crate. This keeps the server's dep weight close to axum + tower + reqwest.

5. **OPA enforcement is opt-in**. `SingleAgent` and `Conductor` accept `Option<Arc<dyn PolicyEngine>>`; if `None`, no policy calls happen (zero-cost). Default builders use the existing `AllowAll` impl from `tako-core`.

6. **PyPythonProvider GIL hand-off**. The Rust impl of `LlmProvider::chat` for `PyPythonProvider` does `Python::attach(|py| -> PyResult<Bound<PyAny>> { let coro = obj.call_method(py, "chat", (...), None)?; pyo3_async_runtimes::tokio::into_future(coro) })`, then awaits the returned Rust future *outside* `Python::attach`. This is the inverse of the Phase-1 `future_into_py` pattern.

7. **Bedrock + Vertex auth never panic on missing credentials at construction time**. Defer auth resolution to first `chat()` call so users can build providers in tests without AWS creds.

8. **No Phase 2.5 features sneaking in**. If during a commit I'm tempted to add Azure OpenAI, Vertex, Vault — stop and defer. The four spec items chosen above are the perimeter.

## Reused existing patterns (don't rewrite)

- The `eventsource-stream` + `async_stream::stream!` pattern from Phase 1's OpenAI/Anthropic providers — reuse for any future SSE shapes (NOT Bedrock).
- The `wiremock`-against-`base_url` test pattern from `tako-providers/openai/tests/chat.rs` — Bedrock tests follow the same shape using the AWS SDK's `endpoint_url` override.
- The `FakeProvider` + scripted-response pattern from `tests/rust/` — reuse for Conductor coordinator/worker tests.
- `tako_governance::pii` regex set — already covers the redaction transforms OPA's `RedactMessages` / `RedactResponse` decisions need.
- `tako_runtime::Principal` task-local — already in place; OPA + compat server both use it for tenant lookup.

## Commit sequence (logical units, in order)

### Step 0 — close Phase 1
1. `maturin develop --release && pytest tests/python && maturin build --release` — confirm wheel builds and the GIL-discipline test passes locally. If anything fails, fix before any new code.

### Phase 1.5 (Python parity)
2. `feat(tako-py): MCP transport bindings (Stdio + StreamableHttp)`
3. `feat(python/tako): mcp.Stdio / mcp.Http real implementations + tests`
4. `feat(tako-orchestrator): SingleAgent accepts MCP transports for tool discovery`
5. `feat(tako-py): PyPythonProvider shim with GIL-correct hand-off`
6. `feat(python/tako): providers.PythonProvider + test_python_provider.py`
7. `feat(tako-governance): wire opentelemetry-otlp BatchSpanProcessor`
8. `feat(python/tako): tracing.Otlp(endpoint, protocol) replaces stub`
9. `test(python): test_otlp.py against in-process collector`

### Phase 2.A — Bedrock provider
10. `chore: scaffold tako-providers/bedrock crate (workspace + deps)`
11. `feat(tako-providers/bedrock): chat via Converse API`
12. `feat(tako-providers/bedrock): streaming via ConverseStream`
13. `test(tako-providers/bedrock): wiremock against canned Converse JSON`
14. `feat(tako-py): bindings for Bedrock provider`

### Phase 2.B — Conductor
15. `feat(tako-orchestrator): DispatchPlan struct + JSON schema for coordinator`
16. `feat(tako-orchestrator): Conductor with worker pool + Semaphore + worker_timeout`
17. `feat(tako-orchestrator): Conductor span emission (orchestrator.dispatch + worker.name)`
18. `test(tako-orchestrator): Conductor end-to-end with FakeProvider workers`
19. `feat(tako-py): bindings for Conductor`
20. `docs(examples): 02_conductor.py`

### Phase 2.C — OPA/regorus
21. `feat(tako-governance): OpaBundle + cached regorus::Engine`
22. `feat(tako-governance): pre_chat / pre_tool / post_chat enforcement points`
23. `feat(tako-orchestrator): wire PolicyEngine into SingleAgent + Conductor`
24. `feat(tako-governance): AuditLog jsonl writer`
25. `test: end-to-end OPA bundle blocks shell.exec for non-admin tenants`
26. `feat(tako-py): bindings for OpaBundle + AuditLog`

### Phase 2.D — tako-compat server
27. `feat(tako-compat): axum skeleton + healthz/readyz`
28. `feat(tako-compat): POST /v1/chat/completions (non-streaming)`
29. `feat(tako-compat): SSE streaming response + tool-call delta format`
30. `feat(tako-compat): bearer-token AuthResolver + per-tenant Principal`
31. `feat(tako-compat): GET /v1/models from registered providers`
32. `feat(tako-py): tako.compat.serve_openai(orch, host, port)`
33. `test(python): test_compat_server.py against the openai SDK`

### Wrap-up
34. `docs(plan): mark Phase 2 + 1.5 complete; document Phase 2.5 follow-ups`
35. `chore: update CHANGELOG [0.2.0]`

(35 commits is realistic given Phase 1 took 21. Some steps may split or combine during execution.)

## Verification (Definition of Done — Phase 2)

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-features -- -D warnings
cargo test --workspace --all-features
cargo audit

# Python
maturin develop --release
pytest -q tests/python                          # smoke + concurrency + mcp + python_provider + compat + otlp
ruff check python/ tests/python/ examples/
mypy python/tako

# Wheel
maturin build --release
pip install target/wheels/tako-*.whl
python -c "import tako; print(tako.__version__)"   # → 0.2.0

# OTel sanity (manual)
docker run -d -p 4317:4317 otel/opentelemetry-collector-contrib
RUST_LOG=tako=debug python examples/01_single_agent.py  # spans visible at collector
```

CI on Linux + macOS + Windows × 3.10–3.13 must pass everything above.

**Phase 2 acceptance** (mirrors spec §18):
- [ ] Conductor recipe runs end-to-end against 2 providers in a test
- [ ] `tako.compat.serve_openai` passes `openai`-SDK conformance tests for chat.completions
- [ ] OPA bundle blocks a forbidden tool call with a recorded audit log
- [ ] Bedrock provider tested with mock servers (`wiremock`)
- [ ] Phase 1.5 follow-ups (MCP Python bindings, PyPythonProvider, OTLP exporter) all live in Python and tested
- [ ] CHANGELOG updated under `## [0.2.0]`

## Phase 2.5 (deferred from this milestone — for the next plan)

- `tako-providers/azure-openai` (Azure-specific URL + api-version + deployment-name routing)
- `tako-providers/vertex` (`gcp_auth` + REST API)
- `VaultResolver`, `AwsSecretsManagerResolver`, `AzureKeyVaultResolver`, `GcpSecretManagerResolver`
- Full mkdocs nav (concepts/, recipes/, API reference) + GitHub Pages deploy via `docs.yml`
- Multi-tenant Redis-backed `BudgetBackend` (originally Phase 4 in spec; bring forward if customer demand)

## Open questions surfacing during execution (not blockers)

- `regorus` 0.9 API surface vs spec's 0.4 — possible incremental migration if the Engine builder changed.
- AWS SDK `aws-sdk-bedrockruntime` 1.130's Converse types vs spec snapshot 1.50 — the `Message` and `ContentBlock` shapes may have evolved.
- `axum` 0.8 streaming response: prefer `Sse` extractor (built-in) vs hand-rolling `Response<Body>` with chunked transfer.
- `openai` Python SDK 1.x vs 2.x conformance differences for our compat server tests.

These are tactical decisions made when the relevant commit lands; they don't change the plan.
