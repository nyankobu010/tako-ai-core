# PLAN — Phase 2.5 (cloud breadth + carry-overs)

> **Status: complete (v0.3.0, 2026-04-29).** All 6 MUST-LAND items
> shipped. See [CHANGELOG.md](CHANGELOG.md) `## [0.3.0]` for the full
> diff.
>
> Successor to [PLAN_PHASE2.md](PLAN_PHASE2.md). Phase 3 (Trinity
> learned routing) is next; see [PLAN.md](PLAN.md).

## Context

Phase 2 (v0.2.0) shipped Conductor + Bedrock + OPA + the OpenAI-compat
HTTP server, but several items from the original Phase 2 spec slipped to
"Phase 2.5" — and v0.2.0 left two `501 Not Implemented` surfaces
(Bedrock streaming, tako-compat SSE). Phase 2.5 closes both gaps and
adds the cloud breadth needed for enterprise deployments: Azure OpenAI
+ Vertex providers, four cloud secret resolvers, and a full mkdocs site
with GitHub Pages deploy.

## What landed

### 2.5A — Azure OpenAI provider

New crate `crates/tako-providers/azure-openai/`. Reuses the OpenAI
chat.completions wire format verbatim (the `tako-providers-openai`
crate exposes `convert` and `stream` as `#[doc(hidden)] pub mod` for
this purpose); only the URL shape and auth header differ. PyO3 binding
`tako._native.AzureOpenAi` + facade `tako.providers.AzureOpenAI`.
4 wiremock tests, 5 Python smoke tests, `examples/09_azure_openai.py`.

### 2.5B — Vertex AI (Gemini) provider

New crate `crates/tako-providers/vertex/`. Talks to
`{location}-aiplatform.googleapis.com` REST endpoints (`generateContent`
+ `streamGenerateContent?alt=sse`). Auth deferred to caller: the
builder takes a pre-resolved OAuth2 access token via `.access_token()`
or `.access_token_env()` — no `gcp_auth` crate dep.

Wire-format conversion handles Gemini's shape:
- `system` messages hoisted to top-level `systemInstruction`.
- `ContentPart::ToolCall` ↔ `functionCall`.
- `ContentPart::ToolResult` correlated by id back to the original tool
  call to recover the function name (Vertex's `functionResponse`
  schema requires it; tako's ToolResult only carries the call id).
- finishReason `STOP` + tool call promoted to `FinishReason::ToolCalls`.

PyO3 binding `tako._native.Vertex` + facade `tako.providers.Vertex`.
5 wiremock tests, 5 Python smoke tests, `examples/10_vertex_gemini.py`.

### 2.5C — Cloud secret resolvers

`tako-governance::secrets` refactored into a module
(`secrets/{mod,vault,aws_sm,azure_kv,gcp_sm}.rs`). All 4 user-confirmed
resolvers shipped:

- `VaultResolver` — KV-v2 via raw reqwest (no `vaultrs` dep). Optional
  `path#field` JSON-pointer suffix.
- `AwsSecretsManagerResolver` — `aws-sdk-secretsmanager` 1.83. Reuses
  the workspace `aws-config` 1.8 already in for Bedrock. Deferred
  credential resolution; constructor never panics on missing AWS env.
- `AzureKeyVaultResolver` — REST via reqwest. Auth deferred to caller
  (pre-resolved AAD bearer token). Optional `name#version` suffix.
- `GcpSecretManagerResolver` — REST via reqwest. Auth deferred.
  Optional `name#version` suffix. Base64-decodes payload.

PyO3 bindings + new facade module `tako.secrets`. 10 wiremock-backed
Rust tests, 7 Python smoke tests, `examples/11_secrets_vault.py`.

### 2.5D — Bedrock streaming

`crates/tako-providers/bedrock/src/stream.rs` maps each
`aws_sdk_bedrockruntime` event variant onto `ChatChunk`s. `MessageStop`
records finish_reason; `Metadata` records usage; the stream always
terminates with one `ChatChunk::End`. Capabilities flag
`supports_streaming` flips to `true`. 5 unit tests against each event
branch, `examples/12_bedrock_streaming.py`.

### 2.5E — tako-compat SSE streaming

`crates/tako-compat/src/sse.rs` reverse-maps `OrchEvent` →
`chat.completion.chunk` JSON. The route uses `axum::response::sse::Sse`
with `KeepAlive::default()`. Falls back to running `run()` and emitting
one synthetic `AssistantText` + `Final` when the orchestrator's
`stream()` isn't natively implemented (today: SingleAgent + Conductor).
The OpenAI Python SDK can't tell the difference — wire format is
identical.

4 sse unit tests, replaced 'returns 501' integration test,
`tests/python/test_compat_streaming.py` with both raw-SSE and
official-SDK conformance cases.

### 2.5F — Full mkdocs nav + Pages deploy

- 7 `concepts/` pages: providers, orchestrators, policy, secrets,
  budgets, tracing, mcp.
- 7 `recipes/` pages: azure_openai, vertex, bedrock,
  openai_compat_server, conductor, opa_policy, secret_resolvers.
- `api/python.md` (mkdocstrings handler over `python/tako`),
  `api/rust.md` (docs.rs link table).
- `mkdocs.yml` moved to repo root with `docs_dir: docs` (Material's
  modern requirement). Material theme: light + dark, navigation.sections,
  search.highlight, full pymdownx extensions.
- `.github/workflows/docs.yml` builds on push to main when
  `docs/` or `python/tako/` change, deploys via
  `actions/deploy-pages@v4`. Repo Pages settings need to be set to
  'GitHub Actions' once post-merge — flag in PR description.
- `mkdocs build --strict` is clean.

## Verification (Definition of Done — Phase 2.5)

```bash
# Rust
cargo fmt --all -- --check          # clean
cargo clippy --workspace --all-targets -- -D warnings   # clean
cargo test --workspace              # 103 tests passing (was 94 in v0.2.0)

# Python
maturin develop --release
pytest -q tests/python              # 39 tests passing (was 30 in v0.2.0)

# Wheel
maturin build --release
python -c "import tako; print(tako.__version__)"   # → 0.3.0

# Docs
pip install mkdocs-material 'mkdocstrings[python]'
mkdocs build --strict               # clean
```

CI on Linux + macOS + Windows × 3.10–3.13 runs all of the above. The new
`docs.yml` workflow runs on `main` only.

## Acceptance checklist

- [x] Azure OpenAI provider passes wiremock + works in orchestrator
- [x] Vertex provider passes wiremock + works in orchestrator + supports
      tool calling
- [x] All 4 cloud secret resolvers compile + have at least one test
      (Vault / Azure KV / GCP SM via wiremock; AWS SM via SDK
      constructor smoke)
- [x] Bedrock streaming returns chunks (no 501)
- [x] tako-compat SSE passes openai-SDK `stream=True` test
- [x] mkdocs builds with `--strict`; GitHub Pages deploy workflow added
- [x] CHANGELOG `## [0.3.0]` complete

## Scope decisions (confirmed with user, 2026-04-29)

- All four cloud secret resolvers landed (Vault + AWS SM + Azure KV +
  GCP SM). Azure KV and GCP SM kept dep weight low by using REST + a
  pre-resolved bearer token (deferred-auth pattern, mirroring Bedrock).
- Redis-backed `BudgetBackend` stayed deferred to Phase 4 — no customer
  ask yet, in-memory backend works for single-process.
- Vertex provider supports tool calling for parity with the rest of
  the provider matrix.

## Phase 3 (next milestone)

- `Trinity` learned router (rule + ONNX) + training harness
- `SelfCaller` bounded recursion (Sakana Fugu Beta pattern)
- Eval harness (deterministic regression suite)
- Real orchestrator-level streaming (replaces tako-compat's
  emulation fallback for `SingleAgent` and `Conductor`)
