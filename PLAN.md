# PLAN — rolling project index

> Per spec §19 rule 1: this is the rolling project plan that future
> Claude Code sessions read on entry. **Each phase owns its own
> `PLAN_PHASE*.md`**; this file is the high-level index + roadmap.
>
> Workflow rules (commit cadence, fmt/clippy/test gates, etc.) live in
> [CLAUDE.md](CLAUDE.md). Architectural rules live in
> [ARCHITECTURE.md](ARCHITECTURE.md).

`tako` is a Rust-core, Python-facade framework for enterprise agentic
systems. The Rust workspace lives under `crates/`, the Python facade
under `python/tako/`, and the wheel target is `crates/tako-py` built
with maturin + PyO3. See [README.md](README.md) for the project
synopsis and quickstart.

## Phase index

| Phase | Version | Status | Plan doc | Changelog |
|-------|---------|--------|----------|-----------|
| 1 — Foundation | v0.1.0 | done (2026-04-28) | [PLAN_PHASE1.md](PLAN_PHASE1.md) | [`## [0.1.0]`](CHANGELOG.md) |
| 2 — Orchestration (+ bundled 1.5) | v0.2.0 | done (2026-04-29) | [PLAN_PHASE2.md](PLAN_PHASE2.md) | [`## [0.2.0]`](CHANGELOG.md) |
| 2.5 — Cloud breadth | v0.3.0 | done (2026-04-29) | [PLAN_PHASE25.md](PLAN_PHASE25.md) | [`## [0.3.0]`](CHANGELOG.md) |
| 3 — Learned coordination | v0.4.0 | done (2026-04-29) | [PLAN_PHASE3.md](PLAN_PHASE3.md) | [`## [0.4.0]`](CHANGELOG.md) |
| 4 — Search & scale | v0.5.0 | done (2026-04-29, retro plan) | [PLAN_PHASE4.md](PLAN_PHASE4.md) | [`## [0.5.0]`](CHANGELOG.md) |
| 5 — Production hardening | v0.6.0 | done (2026-04-29) | [PLAN_PHASE5.md](PLAN_PHASE5.md) | [`## [0.6.0]`](CHANGELOG.md) |
| 6 — Production hardening, continued | v0.7.0 | done (2026-04-29) | [PLAN_PHASE6.md](PLAN_PHASE6.md) | [`## [0.7.0]`](CHANGELOG.md) |
| 7 — Sigstore + streaming closures | v0.8.0 | done (2026-04-29) | [PLAN_PHASE7.md](PLAN_PHASE7.md) | [`## [0.8.0]`](CHANGELOG.md) |
| 8 — Search streaming + transparency-log completeness | v0.9.0 | done (2026-04-29) | [PLAN_PHASE8.md](PLAN_PHASE8.md) | [`## [0.9.0]`](CHANGELOG.md) |
| 9 — Cost-aware streaming guards + log freshness + protocol completeness + router-driven AB-MCTS | v0.10.0 | done (2026-04-30) | [PLAN_PHASE9.md](PLAN_PHASE9.md) | [`## [0.10.0]`](CHANGELOG.md) |
| 10 — Phase 9 follow-on completeness + cross-orchestrator verifier scores + Python provider streaming | v0.11.0 | done (2026-04-30) | [PLAN_PHASE10.md](PLAN_PHASE10.md) | [`## [0.11.0]`](CHANGELOG.md) |
| 11 — Sigstore security hardening + http-generic provider streaming | v0.12.0 | done (2026-04-30) | [PLAN_PHASE11.md](PLAN_PHASE11.md) | [`## [0.12.0]`](CHANGELOG.md) |
| 12 — MCP SSE notifications + HttpGeneric Python facade | v0.13.0 | done (2026-04-30) | [PLAN_PHASE12.md](PLAN_PHASE12.md) | [`## [0.13.0]`](CHANGELOG.md) |
| 13 — Multi-replica `StateStore` + streaming verifier in Trinity | v0.14.0 | done (2026-04-30) | [PLAN_PHASE13.md](PLAN_PHASE13.md) | [`## [0.14.0]`](CHANGELOG.md) |
| 14 — Streaming verifier in Conductor + tako-compat real auth providers | v0.15.0 | done (2026-04-30) | [PLAN_PHASE14.md](PLAN_PHASE14.md) | [`## [0.15.0]`](CHANGELOG.md) |
| 15 — Streaming verifier in AbMcts + tako-compat auth hardening | v0.16.0 | done (2026-05-01) | [PLAN_PHASE15.md](PLAN_PHASE15.md) | [`## [0.16.0]`](CHANGELOG.md) |
| 16 — Streaming-rollout backpressure + tako-compat auth hardening, continued | v0.17.0 | done (2026-05-01) | [PLAN_PHASE16.md](PLAN_PHASE16.md) | [`## [0.17.0]`](CHANGELOG.md) |
| 17 — OIDC introspection completeness | v0.18.0 | done (2026-05-01) | [PLAN_PHASE17.md](PLAN_PHASE17.md) | [`## [0.18.0]`](CHANGELOG.md) |
| 18 — OIDC introspection asymmetric JWT + end-session helper | v0.19.0 | done (2026-05-01) | [PLAN_PHASE18.md](PLAN_PHASE18.md) | [`## [0.19.0]`](CHANGELOG.md) |
| 19 — Vision content support: Anthropic + OpenAI | v0.20.0 | done (2026-05-01) | [PLAN_PHASE19.md](PLAN_PHASE19.md) | [`## [0.20.0]`](CHANGELOG.md) |

Trait surface in `tako-core` is designed so each phase is purely
additive — public APIs from earlier phases never break.

## Roadmap

### Phase 20 candidates (indicative, not yet committed)

Carry-forward from Phase 19's holding pen — Anthropic + OpenAI
vision content landed in Phase 19. The remainder:

- **Vision / image content for Vertex + Mistral + Ollama.**
  Phase 19 wired the two flagship providers (Anthropic + OpenAI);
  the three remaining providers each have a different per-vendor
  multimodal-content shape (Vertex's `inline_data` / `file_data`,
  Mistral's model-specific multimodal, Ollama's LLaVA-family
  embedding). Best handled in a single Phase 20 sweep.
- **Eval harness real graders** (SWE-Bench Lite, GPQA Diamond) —
  promised in Phase 3 PLAN, still raise `NotImplementedError`.
  Sandboxed runner needed.
- **OIDC introspection mTLS auth methods** (`tls_client_auth` /
  `self_signed_tls_client_auth`). Needs client TLS material plumbed
  through `reqwest::ClientBuilder` at workspace scope.
- **OIDC refresh-token / revocation-endpoint flows** — tako as
  token *consumer* rather than validator (different model from
  the existing `AuthResolver` surface).
- **Composite `AuthResolver`s** (mTLS + bearer chaining) —
  orthogonal.

### Beyond (speculative)

- Cosign protobuf-bundle deeper integration (CLI-friendly file inputs;
  full `sigstore-protobuf-specs` migration vs. vendored subset).
- Provider breadth: more open-weight providers, hardware-accel inference
  endpoints.
- Tracing + cost rollup against multi-tenant deployments.
- Eval-driven router fine-tuning loop (Trinity training-from-traces).

### Backlog (uncommitted)

Items surfaced from a 2026-04-30 audit of phase markers across the codebase.
Not yet slotted into a phase; recorded here so they don't get lost between
phase transitions. File/line references point at the stale marker, not at
where the fix would land.

#### Stale phase markers — promised but not delivered

- [x] **MCP Streamable HTTP — SSE upgrade + `Mcp-Session-Id` lifecycle.**
  Closed in Phase 12.A (v0.13.0): `notifications()` opens a long-lived
  `GET {url}` over `text/event-stream`, parses each `data:` line as
  JSON-RPC, broadcasts method-bearing frames to subscribers, attaches
  the latched `Mcp-Session-Id` header on the GET, and shuts down on
  `close()` via a `tokio::sync::Notify`.
- [x] **`tako-providers/http-generic` streaming.** Closed in Phase
  11.B (v0.12.0): set `HttpGenericConfig::stream_config` to a
  `StreamConfig::OpenAiSse` or `StreamConfig::NdJson` variant
  with JSON-pointer-based delta extraction.
- [x] **Python custom provider streaming.** Closed in Phase 10.D
  (v0.11.0): pass `stream=async_gen_fn` to
  `tako.providers.PythonProvider` and the Rust side iterates the
  async generator via `__anext__()`, deserialising each yielded
  dict to a `ChatChunk` via the `kind`-tagged JSON shape.
- [x] **Multi-replica Rekor freshness anchor.** Closed in Phase
  13.A (v0.14.0): a new public
  `tako_governance::sigstore_state::StateStore` async trait plus
  a `RedisStateStore` impl gated behind a `tako-governance/redis`
  cargo feature. A small Lua script enforces monotonic write so a
  slow replica cannot clobber a higher water-mark.
  `tako.sigstore.RedisStateStore` exposes the same surface from
  Python.
- [x] **Streaming-aware `Verifier` in Trinity.** Closed in Phase
  13.B (v0.14.0): `Verifier::evaluate_streaming` default-impl
  method on `tako-core` plus per-delta wiring in
  `Trinity::stream`. `RuleBasedVerifier` overrides the hook so the
  shipped cheap-heuristic verifier drives partial scores out of
  the box.
- [x] **Streaming-aware `Verifier` in Conductor.** Closed in Phase
  14.A (v0.15.0): worker fanout in `Conductor::stream` now drives
  `provider.stream(...)` for streaming-capable workers and surfaces
  per-delta progress as `OrchEvent::VerifierScore { step,
  branch=(idx+1), score }` on the same `(step, branch)` as the
  Phase 10.C synthesis-complete final. Non-streaming workers fall
  back to `chat()` — zero partials, one final per worker (v0.14.0
  parity preserved).
- [x] **`tako-compat` real auth providers — Vault / JWT / OIDC.**
  Closed in Phase 14.B (v0.15.0): three new
  `tako_compat::AuthResolver` impls behind cargo features
  (`jwt` / `oidc` / `vault`), each mirrored as a Python pyclass
  under matching wheel-side `auth-*` features. `JwtAuthResolver`
  pins the algorithm at construction so alg-confusion attacks fail
  closed; `OidcAuthResolver` does discovery + JWKS rotation with a
  one-shot force-refresh on signature failure;
  `VaultAuthResolver` looks up bearer tokens in KV v2 with a
  positive-only TTL cache.
- [x] **Per-delta streaming `Verifier` in AB-MCTS rollouts.** Closed
  in Phase 15.A (v0.16.0):
  [`AbMcts::stream`](crates/tako-orchestrator/src/ab_mcts.rs)
  now branches on `picked.capabilities().supports_streaming` and
  drives `provider.stream(...)` through a new
  `rollout_static_streaming` helper modelled on Trinity (13.B) and
  Conductor (14.A). Per-delta `OrchEvent::VerifierScore` events
  share `(step, branch=leaf_idx)` with the synthesis-complete
  final.
- [x] **Vault dynamic token rotation.** Closed in Phase 15.B.1
  (v0.16.0): new public `VaultTokenProvider` async trait plus
  `StaticVaultToken` / `AppRoleTokenProvider` /
  `KubernetesTokenProvider` impls. `VaultAuthResolver` keeps its
  `new(addr, token)` shape but gains `with_provider`,
  `with_approle`, `with_kubernetes`, and `with_kubernetes_in_pod`
  constructors; Python facade mirrors the new surface.
- [x] **OIDC token introspection (RFC 7662).** Closed in Phase
  15.B.2 (v0.16.0): new public `IntrospectionConfig` struct + two
  `OidcAuthResolver` builders (`with_introspection` /
  `with_introspection_uri`); Python facade mirrors them as
  `OidcAuth.with_introspection_*`. Fail-closed when the issuer
  doesn't advertise the endpoint.
- [x] **Bounded mpsc backpressure for streaming verifier rollouts.**
  Closed in Phase 16.A (v0.17.0): `AbMcts::stream` and
  `Conductor::stream` both swap their per-delta `OrchEvent` /
  `WorkerStreamEvent` channels from `unbounded_channel` to
  `channel(64)` (matching the `tako-mcp/src/transport/grpc.rs`
  precedent). Producers block on `send().await` once the consumer
  is behind, capping in-flight memory under slow `evaluate_streaming`
  impls or slow downstream sinks. Trinity is naturally serial (no
  channel) — no plumbing needed.
- [x] **Vault Enterprise namespace support.** Closed in Phase
  16.B.1 (v0.17.0): `VaultAuthResolver::with_namespace(ns)`
  threads the value through `VaultClientSettingsBuilder::namespace`
  so the cached `VaultClient` sends `X-Vault-Namespace` on every KV
  lookup. `None` (default) preserves OSS-Vault behaviour. Python
  facade mirrors as `VaultAuth.with_namespace`.
- [x] **OIDC introspection `client_secret_post` auth method.**
  Closed in Phase 16.B.2 (v0.17.0): new public
  `IntrospectionAuthMethod` enum (`ClientSecretBasic` default,
  `ClientSecretPost` sibling per RFC 7662 §2.1) plus chainable
  `OidcAuthResolver::with_introspection_auth_method(method)`.
- [x] **OIDC introspection `client_secret_jwt` auth method.**
  Closed in Phase 17.B (v0.18.0): new
  `IntrospectionAuthMethod::ClientSecretJwt` variant signs a
  short-lived HS256 JWT over `client_secret` and sends it as
  `client_assertion` + `client_assertion_type` form fields per
  RFC 7521 / 7523. Asymmetric `private_key_jwt` (RS256 / ES256)
  and mTLS auth methods remain deferred to Phase 18+.
- [x] **Discovery-driven introspection auth-method selection.**
  Closed in Phase 17.A (v0.18.0):
  `OidcAuthResolver::with_introspection_auth_method_from_discovery()`
  reads RFC 8414
  `introspection_endpoint_auth_methods_supported` from the
  discovery doc and picks the strongest mutually-supported method
  (Phase 18.A preference: `private_key_jwt` >
  `client_secret_jwt` > `client_secret_basic` >
  `client_secret_post`). Fail-closed when the issuer advertises
  only methods deferred to Phase 19+ (`tls_client_auth` /
  unknown).
- [x] **OIDC introspection `private_key_jwt` auth method.**
  Closed in Phase 18.A (v0.19.0): new
  `IntrospectionAuthMethod::PrivateKeyJwt` variant signs an
  asymmetric (RS256 / ES256 / EdDSA) JWT client assertion via the
  configured `client_assertion_key`. Same wire shape as
  `ClientSecretJwt` (form-body `client_assertion_type` +
  `client_assertion`, no `Authorization` header). New
  `ClientAssertionKey` struct with typed PEM constructors
  (`from_rs256_pem` / `from_es256_pem` / `from_ed25519_pem`); new
  builder shortcuts `with_introspection_jwt_rs256_pem` /
  `_es256_pem` / `_ed25519_pem`.
- [x] **OIDC end-session endpoint helper.** Closed in Phase 18.B
  (v0.19.0): the discovery doc's `end_session_endpoint` (OIDC
  Session Management 1.0 §2.2.1) is now captured at construction
  time and exposed via `OidcAuthResolver::end_session_endpoint()`
  + `build_logout_uri(id_token_hint, post_logout_redirect_uri,
  state)`. Pure URL building; no I/O.
- [~] **Vision / image content support across providers.**
  Anthropic + OpenAI closed in Phase 19.A / 19.B (v0.20.0).
  Vertex ([convert.rs:202-208](crates/tako-providers/vertex/src/convert.rs#L202-L208)),
  Mistral ([convert.rs:174-178](crates/tako-providers/mistral/src/convert.rs#L174-L178)),
  Ollama ([convert.rs:161-164](crates/tako-providers/ollama/src/convert.rs#L161-L164))
  remain deferred to Phase 20+.
- [ ] **Eval harness real graders.** `swe_bench_lite` and `gpqa_diamond`
  raise `NotImplementedError`; real SWE-Bench (apply patch + run sandboxed
  repo tests) deferred to "a later phase".
  [python/tako/eval/harness.py:9-10](python/tako/eval/harness.py#L9-L10),
  [python/tako/eval/datasets/external.py:8-11](python/tako/eval/datasets/external.py#L8-L11).
- [ ] **OTel end-to-end test against a real gRPC collector.** Full e2e
  test deferred from Phase 1.5 acceptance criteria.
  [tests/python/test_otlp.py:13-16](tests/python/test_otlp.py#L13-L16).
- [ ] **Vertex deterministic-per-call placeholder logic.** Stub flagged
  inline; revisit when usage patterns warrant.
  [crates/tako-providers/vertex/src/convert.rs:291](crates/tako-providers/vertex/src/convert.rs#L291).

#### Documentation maintenance

- [x] **Bring `README.md` feature matrix current.** Phase 9.E
  swept the matrix through Phase 9; Phase 10.E added a Phase 10
  column (verifier scores in Trinity / Conductor; tool-call
  lifecycle named SSE events; on-disk JsonStateStore; Python
  custom provider streaming). Roadmap section enumerates Phases
  1–10.

#### First-publish placeholders

- [ ] **Replace `TODO(<org>)` repository URLs** at first public-org
  publish (intentional single-point-of-edit per `PLAN_PHASE1.md` line 2).
  [Cargo.toml:26](Cargo.toml#L26), [README.md](README.md),
  [CONTRIBUTING.md](CONTRIBUTING.md),
  [CHANGELOG.md:999-1008](CHANGELOG.md#L999-L1008),
  [crates/tako-core/src/lib.rs:8-9](crates/tako-core/src/lib.rs#L8-L9).
- [ ] **Resolve `TODO(community)` placeholders** — Discussions
  categories, Discord/Matrix room ([README.md:135-136](README.md#L135-L136)),
  conduct@ contact (CODE_OF_CONDUCT.md), security@ contact
  (SECURITY.md).

## How to work this index

When opening a new phase: pick the next `Phase N` slot, create
`PLAN_PHASE<N>.md` (mirror the structure of [PLAN_PHASE6.md](PLAN_PHASE6.md)
or [PLAN_PHASE7.md](PLAN_PHASE7.md)), add a row to the table above, and
move "in progress" to that row. When the phase ships, flip the status
to "done (date)" and update the corresponding `CHANGELOG.md` anchor.
