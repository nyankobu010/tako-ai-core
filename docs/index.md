# tako 蛸

> Rust-core, Python-facade framework for enterprise agentic systems.
>
> Many arms, one mind.

`tako` (Japanese: octopus) is an open-source framework for building production
agentic systems. It gives you vendor-neutral provider abstractions, a Rust
orchestration core that keeps Python's GIL out of the hot path, MCP tool
connectivity, and the governance plumbing — OTel tracing, OPA policy, PII
redaction, budgets, circuit breakers — you actually need at scale, all under
a Pythonic dual sync/async API that ships as one `pip install`.

## Inspiration & credit

`tako` is an open-source generalisation of three patterns Sakana AI published,
plus AB-MCTS tree search:

- **TRINITY** — Xu et al., [arXiv:2512.04695](https://arxiv.org/abs/2512.04695)
- **Conductor** — Nielsen et al., [arXiv:2512.04388](https://arxiv.org/abs/2512.04388)
- **Fugu Beta self-recursion** — [Sakana AI blog](https://sakana.ai/fugu-beta/)
- **AB-MCTS** — Inoue et al., [arXiv:2503.04412](https://arxiv.org/abs/2503.04412); reference [TreeQuest](https://github.com/SakanaAI/treequest) (Apache-2.0)

`tako` is an **independent open-source project**. It is not affiliated with,
endorsed by, or sponsored by Sakana AI or any model provider.

## Install

```bash
pip install tako
```

No Rust toolchain required at install time — wheels are prebuilt for
manylinux, musllinux, macOS universal2, and Windows x64/arm64.

## What's inside (current release)

| Area | Today |
|------|-------|
| **Providers** | OpenAI, Anthropic, Azure OpenAI, AWS Bedrock, Google Vertex (Gemini), Mistral, Ollama, plus an `http-generic` template adapter and a `PythonProvider` for pure-Python custom providers. All six SDK-backed providers handle outbound vision content (`ContentPart::Image` and URL-source `ContentPart::ImageUrl`); Bedrock + Ollama use opt-in tako-side URL pre-fetch with full SSRF mitigation (private-IP blocklist + DNS-rebind defence + per-host / wildcard / CIDR allowlist). |
| **Orchestrators** | `SingleAgent`, `Conductor`, `Trinity` (rule-based or ONNX-backed router), `SelfCaller` (bounded recursion), and `AbMcts` (Adaptive Branching MCTS with verifiers and router-driven branch expansion). All of them stream natively via `OrchEvent`. |
| **Streaming** | Native `provider.stream(...)` on every SDK-backed provider; per-delta `OrchEvent::VerifierScore` on Trinity, Conductor, and AbMcts (bounded `mpsc::channel(64)` for backpressure); streaming-aware `ConfidenceGuard` (`RuleBasedGuard` + opt-in `LlmJudgeGuard` per-N-delta). |
| **MCP** | Stdio, Streamable HTTP (with `notifications()` SSE + `Mcp-Session-Id` lifecycle), WebSocket, and gRPC (with mTLS) transports. |
| **OpenAI-compat HTTP server** (`tako-compat`) | Drop-in replacement for OpenAI's `/v1/chat/completions` that re-emits tako's orchestrator events as `tako.*` SSE extensions (`tako.verifier_score`, `tako.recursion`, `tako.tool_call_start`, `tako.tool_call_result`). Pluggable `AuthResolver` with static, JWT, OIDC, and Vault impls plus a composite `ChainedAuthResolver`; OIDC introspection ships every RFC 7662 / RFC 8414 / RFC 8705 auth method including mTLS with explicit cert/key rotation. |
| **Governance** | OPA / Rego policy enforcement (Allow / Deny / RedactMessages / ForceModel / RequireApproval), PII / DLP redaction, OTel tracing with `tako.*` and `gen_ai.*` semconv, in-memory + Redis budgets, circuit breakers, rate limits. |
| **Sigstore** | Tool-catalogue keyed + keyless verification with operator-pinned trust roots, Rekor SET + inclusion-proof + checkpoint freshness anchor (in-memory, on-disk JSON, or Redis-backed `StateStore`), cosign protobuf-bundle support. |
| **Reliability** | Cascade fallback, governor rate-limit, failsafe circuit breaker, exponential-jitter retry. |
| **API surface** | Sync + async dual API; mypy-strict types; full Pydantic v2 facade. |

See the [feature matrix in README.md](https://github.com/nyankobu010/tako-ai-core/blob/main/README.md#feature-matrix) for the per-phase ledger of which capability landed when.

## Where to go next

- **[Quickstart](quickstart.md)** — install, run, and trace your first agent.
- **[Architecture](architecture.md)** — crate graph, sequence diagrams, async + GIL discipline.
- **[Concepts](concepts/providers.md)** — design surface page-by-page.
- **[Recipes](recipes/azure_openai.md)** — end-to-end walkthroughs per integration.
- **[API reference](api/python.md)** — Python module reference (auto-generated from docstrings) and Rust rustdoc pointers.

## Roadmap

`tako` follows the convention "Phase N → v0.{N+1}.0". The current release
ships Phases 1–34 (v0.35.0). Highlights:

| Phase | Version | Theme |
|-------|---------|-------|
| 1 | v0.1.0 | Foundation — traits, runtime, two providers, MCP basics, `SingleAgent`, OTel, wheel |
| 2 | v0.2.0 | `Conductor`, OPA enforcement, OpenAI-compat server, Bedrock |
| 2.5 | v0.3.0 | Azure + Vertex, cloud secret resolvers, Bedrock + SSE streaming, mkdocs site |
| 3 | v0.4.0 | `Trinity` learned routing, `SelfCaller` recursion, eval harness |
| 4 | v0.5.0 | AB-MCTS, Sigstore (keyed), Mistral / Ollama, WebSocket / gRPC MCP, Redis budgets |
| 5 | v0.6.0 | Sigstore keyless, gRPC mTLS, `BudgetTracker` wired into `SingleAgent` |
| 6 | v0.7.0 | Budget through Conductor / Trinity / Judge, pinned chain-of-trust + Rekor SET |
| 7 | v0.8.0 | `SelfCaller::stream`, Rekor inclusion-proof, cosign protobuf-bundle |
| 8 | v0.9.0 | `OrchEvent::VerifierScore` + `Recursion`, `AbMcts::stream` Python facade, Rekor checkpoint, streaming `ConfidenceGuard` |
| 9 | v0.10.0 | Streaming `LlmJudgeGuard`, Rekor freshness anchor, `tako.*` SSE extensions, router-driven AB-MCTS |
| 10 | v0.11.0 | On-disk `JsonStateStore`, tool-call-lifecycle SSE, verifier scores in Trinity / Conductor, Python provider streaming |
| 11 | v0.12.0 | Sigstore review-driven hardening, `http-generic` streaming |
| 12 | v0.13.0 | MCP SSE notifications, `HttpGeneric` Python facade |
| 13 | v0.14.0 | Multi-replica `RedisStateStore`, streaming `Verifier` in Trinity |
| 14 | v0.15.0 | Streaming `Verifier` in Conductor, real `AuthResolver` impls (JWT / OIDC / Vault) |
| 15 | v0.16.0 | Streaming `Verifier` in AbMcts, Vault dynamic token rotation, OIDC introspection |
| 16 | v0.17.0 | Bounded mpsc backpressure, Vault Enterprise namespace, OIDC `client_secret_post` |
| 17 | v0.18.0 | OIDC discovery-driven introspection, `client_secret_jwt` |
| 18 | v0.19.0 | OIDC `private_key_jwt`, end-session helper |
| 19 | v0.20.0 | Vision content on Anthropic + OpenAI |
| 20 | v0.21.0 | Vision content on Vertex + Mistral + Ollama (six-of-six provider sweep) |
| 21 | v0.22.0 | `ChainedAuthResolver` composite |
| 22 | v0.23.0 | URL-source images on Anthropic + OpenAI + Mistral |
| 23 | v0.24.0 | URL-source images on Vertex |
| 24 | v0.25.0 | OIDC introspection mTLS (`tls_client_auth`) |
| 25 | v0.26.0 | OIDC `self_signed_tls_client_auth` |
| 26 | v0.27.0 | `ChainedAuthResolver` transport-error fail-fast |
| 27 | v0.28.0 | `ChainedAuthResolver` broader infrastructure-error fail-fast |
| 28 | v0.29.0 | URL-source images on Bedrock + Ollama via opt-in tako-side pre-fetch |
| 29 | v0.30.0 | URL pre-fetch SSRF hardening (private-IP blocklist + DNS-rebind), Ollama Python facade |
| 30 | v0.31.0 | URL pre-fetch per-host allowlist |
| 31 | v0.32.0 | URL pre-fetch wildcard host patterns |
| 32 | v0.33.0 | URL pre-fetch CIDR allowlist |
| 33 | v0.34.0 | OIDC mTLS cert/key rotation |
| 34 | v0.35.0 | Public-release prep — TODO sweep, OSS hygiene, docs site refresh |

See [`PLAN.md`](https://github.com/nyankobu010/tako-ai-core/blob/main/PLAN.md)
for the rolling project index and per-phase plan documents.

## License

Apache-2.0 — see [`LICENSE`](https://github.com/nyankobu010/tako-ai-core/blob/main/LICENSE)
and [`NOTICE`](https://github.com/nyankobu010/tako-ai-core/blob/main/NOTICE).
