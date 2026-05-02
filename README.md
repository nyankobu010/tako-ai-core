# tako 蛸

> **Rust-core, Python-facade framework for enterprise agentic systems.**
>
> Many arms, one mind.

[![CI](https://github.com/nyankobu010/tako-ai-core/actions/workflows/ci.yml/badge.svg)](https://github.com/nyankobu010/tako-ai-core/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/tako.svg)](https://pypi.org/project/tako/)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

`tako` is an open-source framework for building production agentic systems. It
gives you vendor-neutral provider abstractions, a Rust orchestration core that
keeps Python's GIL out of the hot path, MCP tool connectivity, and the
governance plumbing (OTel tracing, OPA policy, PII redaction, budgets, circuit
breakers) you actually need at scale — all with a Pythonic, dual sync/async
API that ships as one `pip install`.

## Inspiration & credit

`tako` is an open-source generalisation of three patterns Sakana AI published,
plus AB-MCTS tree search:

1. **Trinity-style learned routing** — a small model selects which
   provider/role handles each step. *Xu et al., "TRINITY: An Evolved LLM
   Coordinator,"* [arXiv:2512.04695](https://arxiv.org/abs/2512.04695).
2. **Conductor-style natural-language orchestration** — a coordinator agent
   decomposes tasks and dispatches workers. *Nielsen et al., "Learning to
   Orchestrate Agents in Natural Language with the Conductor,"*
   [arXiv:2512.04388](https://arxiv.org/abs/2512.04388).
3. **Self-recursive test-time scaling** — bounded recursion in which an agent
   reads its own output and decides whether to spin up corrective workflows.
   See Sakana AI's [Fugu Beta](https://sakana.ai/fugu-beta/) blog post.
4. **AB-MCTS** — Adaptive Branching Monte Carlo Tree Search. *Inoue et al.,*
   [arXiv:2503.04412](https://arxiv.org/abs/2503.04412); reference
   implementation by Sakana AI as
   [TreeQuest](https://github.com/SakanaAI/treequest) (Apache-2.0).

> `tako` is an **independent open-source project**. It is not affiliated with,
> endorsed by, or sponsored by Sakana AI or any model provider. The cited
> papers are credited as inspiration for the underlying patterns; the
> implementation is the work of the `tako` contributors. The name `tako`
> ("octopus") complements Sakana AI's "Fugu" (pufferfish) as a tribute.

## Install

```bash
pip install tako
```

No Rust toolchain required at install time — wheels are prebuilt for
manylinux, musllinux, macOS universal2, and Windows x64/arm64.

## Quickstart

```python
import asyncio
import tako

client = tako.Client(
    providers=[
        tako.providers.Anthropic(model="claude-opus-4-7"),
        tako.providers.OpenAI(model="gpt-5"),
    ],
    mcp_servers=[
        tako.mcp.Stdio(command=["npx", "-y", "@modelcontextprotocol/server-everything"]),
    ],
    tracing=tako.tracing.Otlp(endpoint="http://otel-collector:4317"),
    budget=tako.Budget(max_usd_per_request=5.0, max_usd_per_day=500.0),
)

orch = tako.orchestrator.SingleAgent(
    provider="anthropic:claude-opus-4-7",
    max_steps=10,
)

async def main():
    result = await orch.run("What's the weather in Tokyo? Use a tool.")
    print(result.text)

asyncio.run(main())

# Synchronous sibling:
result = orch.run_sync("Quick question: ...")
```

## Capabilities (current release)

| Area | What's available today |
|------|------------------------|
| **Providers** | OpenAI, Anthropic, Azure OpenAI, AWS Bedrock, Google Vertex (Gemini), Mistral, Ollama, plus an `http-generic` template adapter and a `PythonProvider`. All seven SDK-backed providers handle outbound vision content (inline + URL-source); Bedrock + Ollama use opt-in tako-side URL pre-fetch with the full SSRF mitigation stack (private-IP blocklist + DNS-rebind defence + per-host / wildcard / CIDR allowlist). |
| **Orchestrators** | `SingleAgent`, `Conductor`, `Trinity` (rule-based or ONNX-backed router), `SelfCaller` (bounded recursion), `AbMcts` (Adaptive Branching MCTS with verifiers + router-driven branch expansion). All stream natively via `OrchEvent`. |
| **Streaming** | Native `provider.stream(...)` on every SDK-backed provider; per-delta `OrchEvent::VerifierScore` on Trinity, Conductor, AbMcts (bounded `mpsc::channel(64)` for backpressure); streaming-aware `ConfidenceGuard` (`RuleBasedGuard` + opt-in `LlmJudgeGuard` per-N-delta). |
| **MCP** | Stdio, Streamable HTTP (with `notifications()` SSE + `Mcp-Session-Id` lifecycle), WebSocket, gRPC (with mTLS). |
| **OpenAI-compat HTTP server** (`tako-compat`) | Drop-in `/v1/chat/completions` with `tako.*` SSE extensions (`tako.verifier_score`, `tako.recursion`, `tako.tool_call_*`). Pluggable `AuthResolver`: static, JWT, OIDC, Vault, plus a composite `ChainedAuthResolver` with opt-in transport / infrastructure-error fail-fast. OIDC introspection ships every RFC 7662 / 8414 / 8705 auth method including mTLS with explicit cert/key rotation. |
| **Governance** | OPA / Rego policy (Allow / Deny / RedactMessages / ForceModel / RequireApproval), PII / DLP redaction, OTel tracing with `tako.*` + `gen_ai.*` semconv, in-memory + Redis budgets, circuit breakers, rate limits. |
| **Sigstore** | Tool-catalogue keyed + keyless verification with operator-pinned trust roots, Rekor SET + inclusion-proof + checkpoint freshness anchor (in-memory / on-disk JSON / Redis-backed `StateStore`), cosign protobuf-bundle. |
| **Reliability** | Cascade fallback, governor rate-limit, failsafe circuit breaker, exponential-jitter retry. |
| **API surface** | Sync + async dual API; mypy-strict types; full Pydantic v2 facade. |

For the **chronological ledger** of which capability landed in which phase
(33 phases × 17 rows), see [`docs/feature_matrix.md`](docs/feature_matrix.md)
or the [Feature matrix page](https://nyankobu010.github.io/tako-ai-core/feature_matrix/)
on the docs site.

## Roadmap

- **Phase 1 — Foundation** *(done, v0.1.0)*: traits, runtime, two providers,
  MCP basics, `SingleAgent`, OTel, PyO3 wheel, CI green.
- **Phase 2 — Orchestration** *(done, v0.2.0)*: `Conductor`, OPA enforcement,
  OpenAI-compat server, Bedrock provider.
- **Phase 2.5 — Cloud breadth** *(done, v0.3.0)*: Azure OpenAI / Vertex
  providers, Bedrock streaming, OpenAI-compat SSE, cloud secret resolvers,
  full mkdocs nav.
- **Phase 3 — Learned coordination** *(done, v0.4.0)*: `Trinity` router
  (rule + ONNX), training harness, `SelfCaller` recursion, eval harness,
  native orchestrator streaming.
- **Phase 4 — Search & scale** *(done, v0.5.0)*: AB-MCTS with verifiers,
  Mistral / Ollama, WebSocket / gRPC MCP, Sigstore (keyed) verification,
  Redis budget backend.
- **Phase 5 — Production hardening** *(done, v0.6.0)*: Sigstore keyless
  verifier (Fulcio leaf cert + identity policy), gRPC MCP mTLS, and
  `BudgetTracker` orchestrator wiring through `tako.SingleAgent` /
  `tako.Client`.
- **Phase 6 — Production hardening, continued** *(done, v0.7.0)*:
  `BudgetTracker` wired through `tako.Conductor`, `tako.Trinity`, and
  `tako.guards.LlmJudge`; `KeylessVerifier` extended with operator-pinned
  chain-of-trust validation (`TrustRoot`) and Rekor SET verification.
- **Phase 7 — Streaming closures + Sigstore continuation** *(done, v0.8.0)*:
  native `SelfCaller::stream` plus first Python streaming entry point
  (`tako.SelfCaller.stream` + `tako._native.OrchEvent` /
  `OrchEventStream`); Rekor inclusion-proof (Merkle audit-path)
  verification; cosign protobuf-bundle adapter
  (`KeylessBundle::from_protobuf_bundle`).
- **Phase 8 — Search streaming + transparency-log completeness**
  *(done, v0.9.0)*: `OrchEvent::VerifierScore` and
  `OrchEvent::Recursion` variants on a now-`#[non_exhaustive]` enum;
  native `AbMcts::stream` plus `tako.AbMcts(...)` Python facade
  (closes the v0.5.0 binding gap); Rekor checkpoint (`SignedNote`)
  verification; streaming-aware `ConfidenceGuard` with `RuleBasedGuard`
  early-abort on `SelfCaller::stream`.
- **Phase 9 — Cost-aware streaming guards + log freshness + protocol
  completeness + router-driven AB-MCTS** *(done, v0.10.0)*:
  opt-in streaming `LlmJudgeGuard` (`with_streaming_min_chars` /
  `with_streaming_every_n` per-N-delta judging); Rekor checkpoint
  freshness anchor (trust-on-first-use over `tree_size`); `tako-compat`
  named `tako.verifier_score` / `tako.recursion` SSE events for
  OpenAI-compat clients; AB-MCTS router-driven branch expansion
  (`AbMcts::builder().candidate(p).router(r)`).
  (`AbMcts::builder().candidate(p).router(r)`).
- **Phase 10 — Phase 9 follow-on completeness + cross-orchestrator
  verifier scores + Python provider streaming** *(done, v0.11.0)*:
  on-disk `JsonStateStore` for Rekor checkpoint freshness anchor
  (crash-safe atomic JSON persistence; `seed` / `persist`
  convenience wrappers around `KeylessVerifier`); `tako-compat`
  named `tako.tool_call_start` / `tako.tool_call_result` SSE
  extension events (`ToolCallResult` previously had no observable
  representation in the OpenAI mapping); `OrchEvent::VerifierScore`
  for `Conductor` (per-worker, `branch` = 1-based dispatch index)
  and `Trinity` (per-role, `branch` = role's positional index);
  `tako.providers.PythonProvider(stream=async_gen)` closes the
  Phase 2 streaming-stale marker on the Python custom provider.
- **Phase 11 — Sigstore security hardening + `http-generic`
  provider streaming** *(done, v0.12.0)*: review-driven sigstore
  hardening from [`SECURITY_PHASE10.md`](SECURITY_PHASE10.md):
  race-free Rekor checkpoint freshness-anchor advance via
  `compare_exchange_weak`; `JsonStateStore` confidentiality
  (`0o600` mode on Unix; `tempfile::NamedTempFile` for
  collision-free atomic writes; `#[serde(deny_unknown_fields)]` +
  schema `version`); chain-of-trust hardening
  (`BasicConstraints: cA=TRUE` + `pathLenConstraint` + critical-
  extension whitelist in `verify_chain`); SAN-list iteration so
  attacker-injected SAN entries cannot win the predicate; canonical
  SET payload via `BTreeMap`. Plus `tako-providers-http-generic`
  streaming via a new opt-in `StreamConfig` enum (OpenAI-compatible
  SSE + NDJSON variants) with JSON-pointer-based delta extraction.
- **Phase 12 — MCP SSE notifications + `HttpGeneric` Python facade**
  *(done, v0.13.0)*: clears two long-standing debts. (A) MCP
  Streamable HTTP `notifications()` previously returned
  `futures::stream::empty()`; now opens a long-lived
  `GET {url}` over `text/event-stream`, broadcasts method-bearing
  JSON-RPC frames to subscribers via `tokio::sync::broadcast`,
  attaches the `Mcp-Session-Id` header captured from a prior POST,
  and shuts down on `close()` via `tokio::sync::Notify`. (B) The
  Phase 11.B Rust streaming surface for `HttpGenericProvider`
  (chat + streaming via `StreamConfig::OpenAiSse | NdJson`) is now
  reachable from Python: `tako.providers.HttpGeneric(...)` mirrors
  the `Bedrock` / `Vertex` facade pattern, accepts dict-shaped
  `body_template` and `stream_config`, and exposes a
  `supports_streaming` property surfacing
  `Capabilities::supports_streaming`. Both items reuse existing
  patterns (WebSocket transport for SSE, `PyBedrock` for the
  facade); strictly additive — no public API changes shape.
- **Phase 13 — Multi-replica `StateStore` + streaming-aware
  `Verifier` in Trinity** *(done, v0.14.0)*: clears two more
  carry-forward items. (A) New public
  `tako_governance::sigstore_state::StateStore` async trait with
  required `load` / `save` and default-impl
  `seed` / `persist` convenience methods; existing
  `JsonStateStore` (Phase 10.A) implements it via thin
  async-over-sync wrappers; new `RedisStateStore` (gated behind a
  new `tako-governance/redis` cargo feature) keeps a single
  shared key in Redis with monotonic-write Lua-script safety so
  a slow replica cannot clobber a higher water-mark — the
  cross-process analogue of `KeylessVerifier::rekor_max_tree_size`'s
  in-process `fetch_max`. Both stores ship as siblings;
  `tako.sigstore.RedisStateStore` mirrors the Python facade.
  (B) `tako_core::Verifier` gains an optional
  `evaluate_streaming(&self, principal, partial) -> Option<f32>`
  default-impl method (default `Ok(None)`). `Trinity::stream`
  now calls it on each cumulative assistant-text delta and emits
  per-delta `OrchEvent::VerifierScore` events on the same
  `(step, branch)` as the eventual synthesis-complete final;
  consumers distinguish partials from the final by `(step, branch)`
  repetition. The shipped `RuleBasedVerifier` (and
  `tako.verifiers.RuleBased`) overrides the hook out of the box.
  Conductor's worker dispatch is non-streaming today; Conductor
  extension is deferred. Both items strictly additive — public
  APIs unchanged shape.
- **Phase 14 — Streaming `Verifier` in Conductor + tako-compat
  real auth providers** *(done, v0.15.0)*: clears two more
  carry-forwards. (A) `Conductor::stream` now drives
  `provider.stream(...)` for streaming-capable workers (mirroring
  Phase 13.B's Trinity wiring) and surfaces per-delta progress as
  `OrchEvent::VerifierScore { step, branch=(idx+1), score }` on
  the same `(step, branch)` as the existing Phase 10.C
  synthesis-complete final. The refactor introduces an internal
  `WorkerStreamEvent { Delta, Done }` enum and a new
  `dispatch_workers_streaming` free function that owns a
  `tokio::sync::mpsc::UnboundedSender`; the outer `Conductor::stream`
  recv-loop calls `Verifier::evaluate_streaming` on each delta's
  cumulative buffer. Non-streaming workers fall through to
  `provider.chat(...)` — zero partials, one final per worker
  (byte-for-byte parity with v0.14.0). 1-based `branch` identity
  is stamped at task-construction time so it stays stable under
  concurrent worker completion. (B) Three new
  [`tako_compat::AuthResolver`](crates/tako-compat/src/auth/mod.rs)
  impls beyond `StaticTokens`, each behind its own cargo feature
  on tako-compat (`jwt` / `oidc` / `vault`) and matching
  wheel-side feature on tako-py (`auth-jwt` / `auth-oidc` /
  `auth-vault`): `JwtAuthResolver` (HS256 / RS256 / ES256; pins
  algorithm at construction so `alg=none` and HS/RS confusion fail
  closed); `OidcAuthResolver` (discovery + JWKS rotation with
  one-shot force-refresh on signature failure); `VaultAuthResolver`
  (KV v2 lookups with positive-only TTL cache). Mirrored in
  Python as `tako.compat.JwtAuth` / `tako.compat.OidcAuth` /
  `tako.compat.VaultAuth`; `tako.compat.serve_openai` gains an
  `auth=` parameter. Both items strictly additive — public APIs
  unchanged shape.
- **Phase 15 — Streaming `Verifier` in AbMcts + tako-compat auth
  hardening** *(done, v0.16.0)*: clears three more carry-forwards.
  (A) `AbMcts::stream` now drives `provider.stream(...)` for
  streaming-capable rollouts (mirroring Phase 13.B's Trinity wiring
  and Phase 14.A's Conductor wiring) and surfaces per-delta
  progress as `OrchEvent::AssistantText` +
  `OrchEvent::VerifierScore { step, branch=leaf_idx, score }` on
  the same `(step, branch)` as the existing synthesis-complete
  final. New `rollout_static_streaming` helper runs concurrently
  with the outer `try_stream!` block via a
  `tokio::sync::mpsc::unbounded_channel` + `tokio::select!`
  recv-loop. Branch identity = `leaf_idx as u32`, stamped before
  the leaf is pushed so partials and the final share `(step,
  branch)`. Phase 9.D router-driven mode is honoured: capability is
  checked on the **picked** candidate, not the primary; mixed-
  capability pools degrade gracefully. Non-streaming providers
  still produce one full-text `AssistantText` per rollout — byte-
  for-byte parity with v0.15.0. (B) `VaultAuthResolver` gains
  AppRole + Kubernetes auth-method rotation via a new public
  [`VaultTokenProvider`](crates/tako-compat/src/auth/vault_token.rs)
  trait + three impls (`StaticVaultToken`, `AppRoleTokenProvider`,
  `KubernetesTokenProvider`). New `with_provider`, `with_approle`,
  `with_kubernetes`, and `with_kubernetes_in_pod` constructors;
  `new(addr, token)` keeps working. AppRole / Kubernetes providers
  POST directly via `reqwest` (no `vaultrs` dep bump);
  re-authenticate lazily at `0.9 * lease_duration`. Bounded LRU of
  `VaultClient`s (4 entries) keyed on Vault-token-string handles
  rotation without rebuild storms. (C) `OidcAuthResolver` gains RFC
  7662 token introspection via new `IntrospectionConfig` +
  `with_introspection(client_id, secret)` (uses the discovered
  `introspection_endpoint`; **fail-closed** when not advertised) /
  `with_introspection_uri(uri, ...)` (explicit override) builders.
  POSTs `token=<jwt>&token_type_hint=access_token` as URL-encoded
  form data with HTTP Basic auth; `active=false` returns
  `TakoError::Invalid("oidc: token revoked (introspection ...)")`.
  Python facade mirrors all new surfaces. Three items strictly
  additive — public APIs unchanged shape.
- **Phase 16 — Streaming-rollout backpressure + tako-compat auth
  hardening, continued** *(done, v0.17.0)*: production hardening of
  the streaming-verifier and auth surfaces shipped in 13–15. (A)
  `AbMcts::stream`
  ([crates/tako-orchestrator/src/ab_mcts.rs](crates/tako-orchestrator/src/ab_mcts.rs#L484-L496))
  and `Conductor::stream`
  ([crates/tako-orchestrator/src/conductor.rs](crates/tako-orchestrator/src/conductor.rs#L543))
  swap their per-delta `OrchEvent` / `WorkerStreamEvent` channels
  from `tokio::sync::mpsc::unbounded_channel` to bounded
  `mpsc::channel(64)` (matching the
  `tako-mcp/src/transport/grpc.rs`
  `NOTIFICATION_BUFFER` / `OUTBOUND_BUFFER` precedent). Producers
  block on `send().await` once the consumer is behind, capping
  in-flight queue memory under slow `evaluate_streaming` impls or
  slow downstream sinks. Trinity is naturally serial (no channel)
  — no plumbing needed. Two new
  `..._stream_bounded_backpressure_high_delta_count` regression
  tests drive 256 deltas through the 64-slot channel under a
  counting streaming verifier. (B.1) `VaultAuthResolver` gains
  Vault Enterprise namespace support — chainable
  `with_namespace(ns)` builder threads the value through
  [`VaultClientSettingsBuilder::namespace`](https://docs.rs/vaultrs/0.7/vaultrs/client/struct.VaultClientSettingsBuilder.html)
  so the cached `VaultClient` sends `X-Vault-Namespace` on every
  KV lookup. `None` (default) preserves OSS-Vault behaviour
  byte-for-byte. (B.2) `OidcAuthResolver` introspection gains the
  `client_secret_post` auth method per RFC 7662 §2.1 — new public
  `IntrospectionAuthMethod` enum (`#[derive(Default)]`,
  `ClientSecretBasic` default + `ClientSecretPost` sibling),
  `IntrospectionConfig::auth_method` field, chainable
  `with_introspection_auth_method(method)` setter. `introspect()`
  branches on `auth_method`: `Basic` keeps the
  `Authorization: Basic` header; `Post` adds `client_id` /
  `client_secret` form fields and omits the header. (B.3) Python
  facade mirrors:
  `tako.compat.VaultAuth.with_namespace(ns)` and
  `tako.compat.OidcAuth.with_introspection_auth_method("basic" | "post")`
  (case-insensitive aliases; `ValueError` on garbage). All four
  items strictly additive — public APIs unchanged shape.
- **Phase 17 — OIDC introspection completeness** *(done,
  v0.18.0)*: closes the two OIDC introspection auth-method items
  Phase 16.B.2 explicitly deferred. (A) Discovery-driven
  auth-method selection per RFC 8414 — the
  `introspection_endpoint_auth_methods_supported` field of the
  discovery doc is now captured at construction time on
  `OidcAuthResolver`, and the new chainable
  `with_introspection_auth_method_from_discovery()` builder picks
  the strongest mutually-supported method (preference:
  `client_secret_jwt` > `client_secret_basic` >
  `client_secret_post`). Fail-closed when discovery advertised a
  list with no supported variant (issuer requires only
  `tls_client_auth` / `private_key_jwt`, both deferred to Phase
  18+) — the operator notices at builder time rather than at
  HTTP-401 from the introspection endpoint. (B) `client_secret_jwt`
  introspection auth method per RFC 7521 / 7523 — new
  `IntrospectionAuthMethod::ClientSecretJwt` variant builds a
  short-lived HS256 JWT signed over the configured
  `client_secret` (claims: `iss` / `sub` = `client_id`, `aud` =
  `introspect_uri`, `iat`, `exp` = `iat + 30s`, monotonic `jti`)
  and sends it as the `client_assertion` form field alongside
  `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`.
  No `Authorization` header. Errors at request time when
  `client_secret` is `None`. (C) Python facade mirror:
  `tako.compat.OidcAuth.with_introspection_auth_method("jwt")` and
  `tako.compat.OidcAuth.with_introspection_auth_method_from_discovery()`.
  Asymmetric `private_key_jwt` (RS256 / ES256) and mTLS
  (`tls_client_auth`) introspection auth methods remain deferred
  to Phase 18+. All three items strictly additive — public APIs
  unchanged shape.
- **Phase 18 — OIDC introspection asymmetric JWT + end-session
  helper** *(done, v0.19.0)*: clears two more OIDC carry-forward
  items from Phase 17. (A) `private_key_jwt` introspection auth
  method per RFC 7521 / 7523 — new
  `IntrospectionAuthMethod::PrivateKeyJwt` variant signs a
  short-lived asymmetric (RS256 / ES256 / EdDSA) JWT over the
  configured `client_assertion_key` and sends it as the
  `client_assertion` form field, identical wire shape to 17.B's
  `client_secret_jwt`. New `ClientAssertionKey` struct with typed
  PEM constructors (`from_rs256_pem` / `from_es256_pem` /
  `from_ed25519_pem`); `IntrospectionConfig.client_assertion_key:
  Option<Arc<ClientAssertionKey>>` (Arc because `EncodingKey`
  doesn't impl `Clone`). Three convenience builders
  (`with_introspection_jwt_rs256_pem` / `_es256_pem` /
  `_ed25519_pem`) load the PEM AND flip the auth method. The
  17.A auto-selector is extended to a four-tier preference order:
  `private_key_jwt` (only when an asymmetric key is loaded) >
  `client_secret_jwt` > `client_secret_basic` >
  `client_secret_post`. Existing 17.B `build_client_assertion_hs256`
  helper refactored into a single `build_client_assertion(client_id,
  audience, &EncodingKey, Algorithm)` shared between both JWT
  variants. (B) OIDC Session Management 1.0 end-session helper —
  the discovery doc's `end_session_endpoint` field is now
  captured at construction time; new public
  `OidcAuthResolver::end_session_endpoint() -> Option<&str>`
  accessor and `build_logout_uri(id_token_hint,
  post_logout_redirect_uri, state) -> Option<String>` URL builder
  per OIDC Session Management 1.0 §5. Pure URL building; no I/O.
  RFC 3986 conformance: joins with `?` or `&` depending on whether
  the configured endpoint already carries a query string. (C)
  Python facade mirror:
  `tako.compat.OidcAuth.with_introspection_jwt_rs256_pem` /
  `_es256_pem` / `_ed25519_pem`;
  `tako.compat.OidcAuth.with_introspection_auth_method("private_key_jwt")`;
  `tako.compat.OidcAuth.end_session_endpoint()` and
  `build_logout_uri(...)`. mTLS (`tls_client_auth`) introspection
  auth methods, OIDC refresh-token flows, and composite
  `AuthResolver`s remain deferred to Phase 19+. All three items
  strictly additive — public APIs unchanged shape.
- **Phase 19 — Vision content support: Anthropic + OpenAI**
  *(done, v0.20.0)*: closes the long-stale "vision is out of
  scope for Phase 1" markers on the two flagship providers.
  `tako_core::ContentPart::Image { mime, data_b64 }` has shipped
  since Phase 1 and Bedrock has wired it since Phase 2.5; Phase
  19 brings Anthropic + OpenAI to parity. (A) Anthropic adapter
  emits `{"type": "image", "source": {"type": "base64",
  "media_type": "image/jpeg", "data": "<base64>"}}` per
  Anthropic Messages API (new `AnBlock::Image` variant +
  `AnImageSource` struct). (B) OpenAI adapter switches
  `OaMessage.content` from `Option<String>` to
  `Option<OaContent>` — an untagged enum with `Text(String)` and
  `Blocks(Vec<OaContentBlock>)` variants — so the request emits
  the array-shaped content form (`{"type": "image_url",
  "image_url": {"url": "data:..."}}`) only when an image is
  present. Non-vision messages keep byte-for-byte wire shape
  parity with pre-19.B traffic; tool-result messages also keep
  the flat-string shape. Both adapters: (i) accept the four
  MIME types both vendors support (`image/jpeg`, `image/png`,
  `image/gif`, `image/webp`); other types are silently dropped
  to match the empty-text drop policy elsewhere; (ii) normalise
  data-URL prefixes — callers may pass either bare base64 or
  `data:image/...;base64,...` interchangeably. (C) Python
  facade smoke pins the Pydantic `ContentPart` mirror's
  image-field surface so a regression lands in tests before
  user code. Vertex / Mistral / Ollama stay deferred to
  Phase 20+ (Vertex has different `inline_data` / `file_data`
  part shapes; Mistral / Ollama multimodal is model-specific).
  Three items strictly additive — public APIs unchanged shape
  apart from the OpenAI `OaMessage.content` field-type widen.
- **Phase 20 — Vision content support: Vertex + Mistral + Ollama**
  *(done, v0.21.0)*: finishes the vision-content sweep started
  in Phase 19. After Phase 20 every shipped provider adapter
  (Anthropic, OpenAI, Vertex, Bedrock, Mistral, Ollama —
  six of six) handles outbound `ContentPart::Image`. (A) Vertex
  emits `inlineData` parts on the existing `parts` array
  (camelCase to match Gemini's REST convention); new
  `VxPart::InlineData` variant + `VxInlineData` struct with
  `mimeType` / `data` fields. (B) Mistral mirrors OpenAI byte-
  for-byte: `MiMessage.content` widens from `Option<String>` to
  `Option<MiContent>` (`Text(String)` | `Blocks(...)` untagged
  enum); array form emitted only when an image is present.
  Tool-result messages keep the flat-string shape. (C) Ollama
  uses a fundamentally different protocol — a sibling `images:
  Vec<String>` field on `OlMessage` carrying bare base64;
  `content` stays a flat string. `#[serde(skip_serializing_if =
  "Vec::is_empty")]` keeps non-vision messages
  byte-for-byte wire-shape compatible. All three: same four
  supported MIME types as Phase 19 (Vertex / Mistral filter;
  Ollama passes through and lets the model decide); same
  data-URL prefix normalisation. Per-crate copies of
  `strip_data_url_prefix` / `is_supported_*_mime` /
  `build_data_url` helpers — kept per-crate per ARCHITECTURE.md
  hard rules (no cross-provider deps). 16 new unit tests
  (Vertex 5 + Mistral 6 + Ollama 5) including regression pins
  on the byte-for-byte wire-shape preservation for non-vision
  traffic. URL-source images (server-side fetch from
  request-supplied URLs) remain deferred to Phase 21+ — security
  story unresolved. Three items strictly additive — public APIs
  unchanged shape apart from `MiMessage.content` field-type
  widen and `OlMessage.images` field addition (skip-gated).
- **Phase 21 — Composite AuthResolver** *(done, v0.22.0)*:
  closes a long-standing operator gap on the OpenAI-compat HTTP
  server. (A) `ChainedAuthResolver` is a new always-on
  `AuthResolver` impl that wraps N children and tries them in
  append order. The first child to return `Ok` short-circuits
  (pinned by the `chained_first_match_short_circuits` test which
  asserts the second child is **not** called); on all-`Err` the
  last child's error propagates. Any error falls through to the
  next child — transient OIDC transport failures don't strand a
  static-API-key client. Recursive composition works: a chain
  whose child is itself a chain (pinned by `chained_can_nest`).
  Method named `then(child)` not `with(child)` because `with` is
  a Python keyword — `chain.with(...)` would be a SyntaxError;
  `then` matches the JS `Promise.then` / Rust `Future` `.then(...)`
  idiom for sequential composition. (B) `tako.compat.ChainedAuth`
  (always-on; no cargo feature gate) mirrors the Rust API with
  `__init__()` / `then(child)` / `__len__()`. The
  `extract_auth_resolver` helper at the `serve_openai(auth=...)`
  boundary gains a fourth always-on `cast::<PyChainedAuth>` arm.
  Common pattern: `auth=ChainedAuth().then(oidc).then(jwt)` to
  accept either an OIDC bearer or a static-key-signed JWT.
  Eight new Rust unit tests + six new Python tests; strictly
  additive — public APIs unchanged shape.
- **Phase 22 — URL-source images: Anthropic + OpenAI + Mistral**
  *(done, v0.23.0)*: closes the long-deferred URL-source-image
  gap. Phases 19 + 20 framed the deferral as "server-side fetch
  needs a security story", but that concern only applies when
  *tako* fetches the URL. For the three vendors whose API
  servers fetch URLs themselves (Anthropic, OpenAI, Mistral),
  the security posture is identical to a direct vendor call from
  the user's browser. (A) New
  `tako_core::ContentPart::ImageUrl { url, mime: Option<String> }`
  variant; six provider adapters gain exhaustive match arms.
  Vertex / Bedrock / Ollama silent-drop — Vertex needs
  vendor-specific URI schemes (`gs://...`), Bedrock + Ollama
  would require tako-side pre-fetch (back to the SSRF concern).
  (B) Anthropic refactors `AnImageSource` from struct to
  `#[serde(tag = "type")]` enum with `Base64` + `Url` variants;
  Phase 19.A's wire shape on the Base64 path is byte-for-byte
  preserved (regression-pinned). Per Anthropic Messages API:
  `{"type": "url", "url": "https://..."}`. (C) OpenAI + Mistral
  pass URLs directly to `image_url.url` — no `data:` prefix
  wrapping (regression-pinned: `image_url_does_not_get_data_url_wrapped`).
  Mistral's vision API is OpenAI-compatible so the two adapters
  share the same shape. (D) Python facade — `ContentPart` Pydantic
  model gains an explicit `url: str | None` field for type
  checking + IDE completion. The optional `mime` hint round-trips
  through Python but is intentionally dropped on the Rust side
  before serialisation (none of the three vendors accept it).
  Eleven new Rust tests (Anthropic 4 + OpenAI 3 + Mistral 2) +
  eight new Python tests; strictly additive — public APIs
  unchanged shape apart from the `AnImageSource` struct→enum
  refactor (provider-internal type; wire shape preserved).
- **Phase 23 — URL-source images: Vertex (Gemini fileData)**
  *(done, v0.24.0)*: extends Phase 22's URL-source-image work
  to Vertex. Phase 22 framed Vertex's deferral as "fileData
  accepts only vendor-specific URI schemes"; the Gemini docs
  actually say `fileData` accepts `gs://` GCS URIs, `https://`
  public web URLs (Google fetches both server-side), and Vertex
  File API URIs (out of scope; needs a separate upload helper).
  Per Gemini docs `mimeType` is REQUIRED on `fileData` — the
  optional `mime` from `ContentPart::ImageUrl` is required for
  the Vertex path; mime-less URL-source content silently drops
  (matches the empty-text drop policy elsewhere). New
  `VxPart::FileData` variant + `VxFileData` struct with `mimeType`
  / `fileUri` fields (camelCase wire-shape matches the existing
  `inlineData` / `functionCall` cadence). Five new unit tests
  including a `gs://` URI variant, an `https://` URI variant
  (confirms identical pass-through to GCS), the mime-missing
  silent-drop, the unsupported-MIME silent-drop, and a mixed
  inline + URL coexistence test. The Phase 20.A inline-data
  tests pass byte-for-byte unchanged. After Phase 23, four of
  six provider adapters (Anthropic + OpenAI + Mistral + Vertex)
  handle URL-source images; Bedrock + Ollama remain deferred to
  Phase 24+ (both would need tako-side pre-fetch with an SSRF
  guard — different design problem). Strictly additive — public
  APIs unchanged shape (the `VxPart` enum is `#[serde(untagged)]`
  so the new variant is wire-invisible).
- **Phase 24 — OIDC introspection mTLS (`tls_client_auth`)**
  *(done, v0.25.0)*: closes the OIDC introspection mTLS gap
  deferred since Phase 16 with the framing "needs reqwest TLS
  feature changes at workspace scope". That framing was wrong —
  the existing workspace reqwest features
  (`["rustls", "webpki-roots", ...]`) already expose
  `reqwest::Identity::from_pem`. Phase 24 implements RFC 8705
  mTLS without any workspace-level dep change. After Phase 24
  the OIDC introspection auth-method surface covers all five
  RFC 7662 §2.1 / RFC 8414-listed methods tako ships:
  `client_secret_basic` / `_post` / `_jwt` / `private_key_jwt` /
  `tls_client_auth`. (A) New
  `IntrospectionAuthMethod::TlsClientAuth` variant; new
  `IntrospectionConfig.mtls_client: Option<Arc<reqwest::Client>>`
  field;
  `OidcAuthResolver::with_introspection_mtls(cert_pem, key_pem)`
  builder that loads cert + key, builds a per-resolver
  mTLS-enabled `reqwest::Client` via
  `reqwest::Identity::from_pem`, and flips `auth_method` to
  `TlsClientAuth`. PEM parse / `Client` build failures surface
  as `TakoError::Invalid` at builder time so operators notice
  early. The auto-selector extends to a five-tier preference
  order with `tls_client_auth` at the head when the issuer
  advertises it AND an mTLS identity is configured — mTLS is
  the strongest method (the private key never leaves the
  client; the cert binds to a DN / SAN the issuer
  pre-registered). `introspect()` swaps to the mTLS Client for
  `TlsClientAuth`; body is credential-free, no `Authorization`
  header (the issuer authenticates via the TLS handshake's
  cert, not via body or headers). (B) Python facade:
  `OidcAuth.with_introspection_mtls(cert_pem, key_pem)` +
  `with_introspection_mtls_combined(combined_pem)`;
  `with_introspection_auth_method` accepts three new
  case-insensitive aliases (`"tls_client_auth"` /
  `"tls-client-auth"` / `"mtls"`). Seven new Rust unit tests +
  four new Python tests; strictly additive — public APIs
  unchanged shape. End-to-end mTLS-handshake integration tests
  (real TLS server requiring client auth) deferred to Phase
  26+; the actual mTLS connection is exercised in real
  deployments. RFC 8705 §2.2 `self_signed_tls_client_auth`
  corner case landed in Phase 25.
- **Phase 25 — OIDC `self_signed_tls_client_auth` (RFC 8705
  §2.2)** *(done, v0.26.0)*: closes the OIDC introspection
  auth-method surface to all six published variants. Phase 24
  shipped CA-backed mTLS (`tls_client_auth`); Phase 25 adds the
  self-signed sibling — wire-identical (both present a TLS
  client cert), but the issuer matches the cert directly
  against a pre-registered thumbprint or public-key fingerprint
  instead of a CA chain. Issuers advertise these as separate
  `introspection_endpoint_auth_methods_supported` discovery-list
  entries; the auto-selector treats them as distinct. (A) New
  `IntrospectionAuthMethod::SelfSignedTlsClientAuth` variant +
  `OidcAuthResolver::with_introspection_self_signed_mtls(cert_pem,
  key_pem)` builder + combined-PEM convenience method. The
  `mtls_client` field on `IntrospectionConfig` is reused; both
  mTLS variants build identical `reqwest::Identity::from_pem`
  clients. The Phase 24 five-tier auto-selector extends to a
  six-tier preference order with `tls_client_auth` (CA-backed)
  preferred over `self_signed_tls_client_auth` because the CA
  chain provides ongoing trust validation (revocation, etc.).
  When only `self_signed_tls_client_auth` is advertised, the
  auto-selector picks it. (B) Python facade:
  `OidcAuth.with_introspection_self_signed_mtls(...)` +
  `_combined(...)`; `with_introspection_auth_method` accepts
  four new case-insensitive aliases
  (`"self_signed_tls_client_auth"`,
  `"self-signed-tls-client-auth"`, `"self_signed_mtls"`,
  `"self-signed-mtls"`). After Phase 25 the OIDC introspection
  auth-method surface covers all six RFC 7662 §2.1 / RFC 8414 /
  RFC 8705-listed methods tako ships — natural close-out of the
  ~10-phase OIDC hardening arc that started with Phase 14.B.
  Six new Rust unit tests + four new Python tests; strictly
  additive — public APIs unchanged shape.
- **Phase 26 — `ChainedAuthResolver` fail-fast on transport
  errors** *(done, v0.27.0)*: closes the Phase-21-deferred
  operator-UX issue with the fall-through-on-any-Err default.
  The Phase 21 `ChainedAuthResolver` falls through to the next
  child on any `Err`; for the common `OidcAuth().then(StaticTokens)`
  pattern, this means an unreachable OIDC issuer surfaces as a
  misleading `"unknown bearer token"` from the StaticTokens
  fallback (because the user's OIDC token isn't in the static
  map). Phase 26 adds an opt-in flag so transport errors halt
  the chain immediately, surfacing the actionable
  `"transport error: oidc unreachable"` instead. (A) New
  `ChainedAuthResolver::with_short_circuit_on_transport_error()`
  builder method (idempotent); when enabled, a
  `TakoError::Transport(_)` from any child halts the chain.
  Other variants (`Invalid` / `PolicyDenied` — auth decisions
  the next resolver might overturn) continue to fall through.
  Default `false` preserves Phase 21 fall-through-on-any-Err
  semantics byte-for-byte (regression-pinned). (B) Python
  facade: `ChainedAuth.with_short_circuit_on_transport_error()`
  + `short_circuits_on_transport_error()` accessor. Five new
  Rust unit tests + five new Python tests; strictly additive —
  public APIs unchanged shape. Broader infrastructure-error
  semantics (`RateLimited` / `CircuitOpen` / `BudgetExhausted`
  / `Provider` source-error) deferred to Phase 27+ pending
  per-variant analysis.
- **Phase 27 — `ChainedAuthResolver` broader infrastructure-error
  short-circuit** *(done, v0.28.0)*: closes the Phase-26
  carry-forward. Phase 26 noted: "Other infrastructure-ish
  variants (`RateLimited`, `CircuitOpen`, `BudgetExhausted`, the
  `Provider` source-error case) would also benefit but warrant
  case-by-case analysis." Phase 27's analysis: short-circuit on
  the four "definitely infrastructure / operator-set guard"
  variants where falling through would either mask a non-auth
  failure or circumvent an operator-set cap. (A) New
  `ChainedAuthResolver::with_short_circuit_on_infrastructure_errors()`
  builder method covering `Transport` ∪ `RateLimited` ∪
  `CircuitOpen` ∪ `BudgetExhausted`. The Phase-26 narrower flag
  internally upgrades from a `bool` to a `ShortCircuitPolicy`
  enum (`None` / `TransportOnly` / `AllInfrastructure`); the
  enum is private so public-API churn is zero. Last-write-wins
  between the Phase-26 and Phase-27 builders — the policy is
  overwritten, not merged. Auth-decision errors (`Invalid`,
  `PolicyDenied`) and vendor errors (`Provider`) continue to
  fall through; `Provider` short-circuit defers pending finer
  discrimination on the embedded error. (B) Python facade:
  `ChainedAuth.with_short_circuit_on_infrastructure_errors()` +
  `short_circuits_on_infrastructure_errors()` accessor. The
  Phase-26 `short_circuits_on_transport_error()` accessor now
  returns `True` for both narrower and broader policies (both
  short-circuit on `Transport`). Eight new Rust unit tests + six
  new Python tests; strictly additive — public APIs unchanged
  shape. The Phase-21 + Phase-26 chained tests pass byte-for-
  byte unchanged through the policy enum refactor.
- **Phase 28 — URL-source images: Bedrock + Ollama via opt-in
  tako-side pre-fetch** *(done, v0.29.0)*: closes the
  URL-source-image gap on the two providers Phase 22 + Phase 23
  explicitly deferred. Anthropic + OpenAI + Mistral (Phase 22) +
  Vertex (Phase 23) had vendor-side fetch — `ContentPart::ImageUrl`
  forwards to a vendor-specific URL field and the vendor's
  servers fetch. Bedrock's `ImageSource` has no URL variant,
  and Ollama's `images: Vec<String>` field requires bare base64;
  both vendors' wire formats simply don't accept a URL. The only
  way to ship URL-source images for those two is for tako to
  pre-fetch the bytes itself — which Phase 22 + 23 deferred
  pending an SSRF mitigation design. Phase 28 ships that design
  with security-conscious defaults: opt-in default-off
  (`url_prefetch=false`), `https`-only (operator opt-in via
  `url_prefetch_allow_http=true`), 10-second default timeout
  (override via `url_prefetch_timeout_secs`), 10 MiB default size
  cap (override via `url_prefetch_max_bytes`; checked both via
  `Content-Length` pre-flight and post-fetch byte-count
  defence-in-depth), MIME validation against the four supported
  types (`image/{jpeg,png,gif,webp}` — same surface as Phases 19
  / 20 / 22). CIDR-block egress filtering and DNS-rebinding
  mitigation are explicitly NOT in scope — operators must enforce
  network egress at the deployment level (VPC egress rules,
  Pod-level egress NetworkPolicies, etc). (A)
  [`tako_providers_bedrock`](crates/tako-providers/bedrock):
  new `url_prefetch.rs` helper (`UrlPrefetchConfig::rewrite`
  pre-pass on each `ChatRequest` rewrites
  `ContentPart::ImageUrl { url, mime }` → `ContentPart::Image
  { mime, data_b64 }` in place; uses
  `aws_smithy_types::base64::encode` already in the dep tree).
  `BedrockBuilder::with_url_prefetch_*` methods + `Inner.url_prefetch:
  Option<UrlPrefetchConfig>` field; `chat()` and `stream()` call
  `prefetch.rewrite(&mut req).await?` before convert. Eight new
  unit tests + four wiremock integration tests. (B)
  [`tako_providers_ollama`](crates/tako-providers/ollama): mirror
  structure with per-crate copy of helpers (per ARCHITECTURE.md
  hard rule — no cross-provider deps); `base64` workspace dep
  added. Three new unit tests. (C)
  [`tako-py`](crates/tako-py)
  Python facade: `tako.providers.Bedrock(url_prefetch=...,
  url_prefetch_allow_http=..., url_prefetch_timeout_secs=...,
  url_prefetch_max_bytes=...)` mirrors the Rust builder; signature
  smoke test pins the four kwargs + their defaults. Ollama has no
  Python binding so the Python surface is Bedrock-only. After
  Phase 28 every shipped provider adapter (Anthropic + OpenAI +
  Mistral + Vertex + Bedrock + Ollama — six of six) handles
  URL-source `ContentPart::ImageUrl`. Three items strictly
  additive — public APIs unchanged shape.
- **Phase 29 — URL pre-fetch SSRF hardening + Ollama Python
  facade** *(done, v0.30.0)*: closes the Phase 28-deferred
  CIDR-block + DNS-rebinding mitigation gap, and the Phase 28.C
  asymmetry where `url_prefetch` was threaded through Bedrock
  but not Ollama. The Phase 28 SSRF mitigations were
  `https`-only / timeout / size cap / MIME validation;
  operators were left to enforce network egress at deployment
  level. Without those mitigations, an attacker who can inject
  `ContentPart::ImageUrl` could ask tako to fetch
  `https://169.254.169.254/...` (cloud-instance metadata),
  internal services on RFC 1918, or services bound to loopback.
  Phase 29 closes that with defence-in-depth at two layers:
  (A) [`tako_providers_bedrock`](crates/tako-providers/bedrock):
  new `is_blocked_ip(&IpAddr)` helper rejects loopback (127/8,
  ::1), RFC 1918 (10/8, 172.16/12, 192.168/16), link-local
  (169.254/16, fe80::/10), unspecified, multicast / reserved
  (224/4, 240/4, ff00::/8), IPv6 unique-local (fc00::/7), and
  IPv4-mapped variants (::ffff:127.0.0.1 etc) — pure stdlib;
  `Ipv6Addr::to_ipv4_mapped` stable on workspace MSRV 1.85. New
  `BlocklistResolver` impl of `reqwest::dns::Resolve` validates
  EVERY returned `SocketAddr` (closes the DNS-rebinding window —
  there's no second resolution between validation and
  connection). New inline IP-literal check in `fetch_one` after
  URL parse since reqwest skips the DNS resolver when the host
  is already an IP literal. New
  `BedrockBuilder::with_url_prefetch_allow_private_ips()`
  builder method opts out for deployments where the network
  layer already filters egress; default `block_private_ips:
  true`. Thirteen new unit tests + 2 wiremock integration
  tests. (B)
  [`tako_providers_ollama`](crates/tako-providers/ollama):
  per-crate copy of all 29.A surfaces (per ARCHITECTURE.md hard
  rule — provider crates depend only on `tako-core` + their
  vendor SDK + reqwest; never on each other). Phase 28.B
  established the duplication; Phase 29.B simply extends each
  copy. Same test surface as 29.A. (C)
  [`tako-py`](crates/tako-py) Python facade: new
  `crates/tako-py/src/py_ollama.rs` mirrors
  `py_bedrock.rs` cadence (synchronous build, no AWS-specific
  kwargs); `tako.providers.Ollama` exposes `model`, `base_url`,
  `timeout_secs`, plus the five url_prefetch_* knobs.
  `PyBedrock::new` gains the new `url_prefetch_allow_private_ips`
  kwarg between `url_prefetch_allow_http` and
  `url_prefetch_timeout_secs`. After Phase 29.C both
  URL-prefetching providers (Bedrock + Ollama) have full
  Python parity. Three items strictly additive — public APIs
  unchanged shape apart from the new always-keyword kwarg on
  `PyBedrock::new` (default `false`; existing callers
  unaffected).
- **Phase 30 — URL pre-fetch per-host allowlist** *(done,
  v0.31.0)*: closes the Phase-29-deferred operator-UX gap where
  the binary `with_url_prefetch_allow_private_ips()` flag is a
  sledgehammer. Operators with an internal artifact registry on
  a private RFC 1918 address would have to disable the WHOLE
  blocklist (incl. the canary 169.254.169.254 cloud-metadata
  endpoint) just to permit one trusted host. Phase 30 adds a
  per-host BYPASS: allowlisted hostnames skip ONLY the
  private-IP blocklist for that host; scheme / timeout / size
  cap / MIME validation all still apply (defence-in-depth).
  Three sub-items:
  (A) [`tako_providers_bedrock`](crates/tako-providers/bedrock):
  new `UrlPrefetchConfig.allow_hosts: Arc<HashSet<String>>`
  threaded through the `BlocklistResolver` and the inline
  IP-literal check; new
  `BedrockBuilder::with_url_prefetch_allow_host(host)` chainable
  builder. Match is exact-string against the URL's host
  component (no wildcards in Phase 30). 5 new unit tests.
  (B) [`tako_providers_ollama`](crates/tako-providers/ollama):
  per-crate copy of all 30.A surfaces (per ARCHITECTURE.md hard
  rule). Same test surface as 30.A.
  (C) [`tako-py`](crates/tako-py) Python facade:
  `tako.providers.Bedrock` + `tako.providers.Ollama` both gain
  a `url_prefetch_allow_hosts: list[str] | None` kwarg
  (positioned between `url_prefetch_allow_private_ips` and
  `url_prefetch_timeout_secs`). `None` (default) means empty
  allowlist; existing callers unaffected. Six new Python
  signature smoke tests.
  After Phase 30 the tako-side URL pre-fetch surface offers a
  full SSRF mitigation stack with an operator-grade allowlist
  override. Wildcard host patterns and CIDR allowlists remain
  Phase 31+ candidates.
- **Phase 31 — URL pre-fetch wildcard host patterns** *(done,
  v0.32.0)*: closes the Phase-30-deferred operator-UX gap where
  exact-string allowlist entries had to enumerate every
  subdomain. Phase 31 adds wildcard suffix patterns: a single
  `*.internal.corp` entry now covers all current AND future
  subdomains under that suffix.
  Wildcard semantics: `*.X` matches any hostname ending in `.X`
  (literal `ends_with` check), INCLUDING multi-level subdomains
  (`*.internal.corp` matches both `registry.internal.corp` and
  `staging.images.internal.corp`). Does NOT match the bare apex
  (`internal.corp`); operators add the apex as a separate exact
  entry if needed. Multi-level matching is the operator-intent
  default — RFC 6125's strict one-level semantics is for TLS
  cert SANs, not operator-controlled allowlists.
  Implementation: new `AllowList` struct (per-crate copy on
  Bedrock + Ollama, per ARCHITECTURE.md hard rule) splits
  exact-match hostnames from wildcard suffix patterns at config
  time. Runtime check is a single `HashSet::contains` plus a
  short linear scan over dotted suffixes — no per-call
  `format!` allocation. Phase 30 callers unaffected: existing
  exact-string entries (no `*.` prefix) flow into the `exact`
  HashSet and behave identically.
  Three sub-items:
  (A) [`tako_providers_bedrock`](crates/tako-providers/bedrock):
  `AllowList` struct + `Arc<HashSet<String>>` →
  `Arc<AllowList>` swap on `UrlPrefetchConfig` and
  `BlocklistResolver`. Builder doc updated to document both
  modes. 8 new unit tests.
  (B) [`tako_providers_ollama`](crates/tako-providers/ollama):
  per-crate copy of all 31.A surfaces. Same test surface.
  (C) [`tako-py`](crates/tako-py) Python facade: no PyO3 code
  change (kwarg already accepts arbitrary strings). Both
  `tako.providers.Bedrock` and `tako.providers.Ollama`
  docstrings now document the two match modes (exact + wildcard
  suffix), the multi-level matching semantic, and the bare-apex
  caveat. Six new Python signature smoke tests pin the
  documentation surface. After Phase 31, the URL pre-fetch
  allowlist matches both exact strings (Phase 30) and wildcard
  suffixes (Phase 31). Strictly additive — public APIs
  unchanged shape.
- **Phase 32 — URL pre-fetch CIDR allowlist** *(done,
  v0.33.0)*: closes the Phase-31-deferred operator-UX gap
  where allowlists could only match host strings. Operators
  with private subnets hosting many dynamic hosts — or raw IP
  literals with no DNS at all — now get a CIDR-based bypass:
  `with_url_prefetch_allow_cidr("10.0.5.0/24")` permits any IP
  in that subnet whether reached via hostname resolution or as
  an IP literal in the URL. After Phase 32 the operator
  allowlist surface covers three semantic forms:
  - Exact string  (`"registry.corp"`)     — URL host string
  - Wildcard      (`"*.internal.corp"`)   — URL host suffix
  - CIDR subnet   (`"10.0.5.0/24"`)       — Resolved IP (any)
  Implementation: new `ipnet = "2"` workspace dep (small,
  well-maintained, zero transitive deps). `AllowList` gains
  `cidrs: Vec<IpNet>` field; constructor renamed
  `from_strings` → `from_strings_and_cidrs(hosts, cidrs)`
  with parse-time CIDR validation (`TakoError::Invalid` on
  parse failure). New `AllowList::contains_ip(&IpAddr)` method.
  `BlocklistResolver` and the inline IP-literal check both
  honour CIDRs — bypass triggers when EITHER the host string
  is allowlisted OR a resolved IP falls inside an allowlisted
  CIDR.
  Three sub-items:
  (A) [`tako_providers_bedrock`](crates/tako-providers/bedrock):
  `AllowList.cidrs` field + `with_url_prefetch_allow_cidr(cidr)`
  builder. 8 new unit tests (IPv4/IPv6 match/no-match,
  single-host /32, invalid-CIDR parse error, three-mode
  coexistence, end-to-end wiremock).
  (B) [`tako_providers_ollama`](crates/tako-providers/ollama):
  per-crate copy of all 32.A surfaces. Same test surface.
  (C) [`tako-py`](crates/tako-py) Python facade: both
  `tako.providers.Bedrock` and `tako.providers.Ollama` gain a
  `url_prefetch_allow_cidrs: list[str] | None` kwarg. Class
  docstrings rewritten to describe the three allowlist forms
  side-by-side. Seven new Python signature smoke tests.
  After Phase 32, the operator allowlist arc is closed.
  Strictly additive — public APIs unchanged shape.
- **Phase 33 — OIDC mTLS cert/key rotation** *(done,
  v0.34.0)*: closes the Phase-24/25-deferred operator-UX gap
  where the mTLS introspection client was built once at
  builder time and required a process restart to refresh.
  Phase 33 adds an explicit-reload primitive: operators call
  `OidcAuthResolver::reload_mtls_identity(cert_pem, key_pem)`
  from their own scheduler (cert-manager webhook, Vault PKI
  rotation, filesystem watcher, periodic poll) and the next
  request uses the new identity. The swap is atomic from the
  request-handler's perspective — concurrent introspection
  POSTs either see the old Client or the new one, never a torn
  state. Two sub-items:
  (A) [`tako-compat`](crates/tako-compat) — new internal
  `MtlsClient` newtype wraps an `RwLock<Arc<reqwest::Client>>`;
  the Phase 24/25 builders construct via `MtlsClient::new`;
  `introspect()` snapshots via `MtlsClient::current()` (Arc
  clone under brief read-lock, poison-recoverable); new
  `OidcAuthResolver::reload_mtls_identity()` and `_combined()`
  methods atomically swap the inner Client via `&self`
  (interior mutability lives on the `MtlsClient::inner`
  RwLock — operators call through `Arc<OidcAuthResolver>`
  without exclusive access). Reload errors when no prior
  `with_introspection_mtls` call (operator notices early; not
  silent no-op); reload PEM/Client failures preserve the
  previously installed Client (no partial-rollback). Field
  type widening on `IntrospectionConfig.mtls_client`:
  `Option<Arc<reqwest::Client>>` → `Option<Arc<MtlsClient>>`
  (the only public-API change, affecting only callers who
  construct `IntrospectionConfig` directly with `Some(...)`).
  Seven new unit tests pinning the swap semantics, error
  paths, combined-PEM convenience, and self-signed parity.
  (B) [`tako-py`](crates/tako-py) — new
  `OidcAuth.reload_mtls_identity(cert_pem, key_pem)` and
  `_combined(combined_pem)` Python methods mirror the Rust
  surface. Mutate state in place via internal mutability (no
  immutable-builder pattern). Three new Python smoke tests.
  Trait-based `MtlsIdentityProvider`, automatic refresh-on-
  handshake-failure, and filesystem watcher integration remain
  Phase 34+ candidates.

See [`PLAN.md`](PLAN.md) and [`ARCHITECTURE.md`](ARCHITECTURE.md) for details.

## Community

- Issues: <https://github.com/nyankobu010/tako-ai-core/issues>
- Discussions: <https://github.com/nyankobu010/tako-ai-core/discussions>
- Security: see [`SECURITY.md`](SECURITY.md) — please use GitHub *Private Vulnerability Reporting* rather than opening a public issue.
- Good first issues: <https://github.com/nyankobu010/tako-ai-core/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22>

## License

Apache-2.0 — see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
