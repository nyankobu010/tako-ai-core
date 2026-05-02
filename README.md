# tako 蛸

> **Rust-core, Python-facade framework for enterprise agentic systems.**
>
> Many arms, one mind.

[![CI](https://github.com/nyankobu010/tako-ai-core/actions/workflows/ci.yml/badge.svg)](https://github.com/nyankobu010/tako-ai-core/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/tako-ai-core.svg)](https://pypi.org/project/tako-ai-core/)
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
pip install tako-ai-core
```

The Python import name is still `tako` (`import tako`); the PyPI
distribution is `tako-ai-core` because the bare `tako` slot was
already taken by an unrelated 2011-era project.

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

## Project history

The project ships in numbered phases. The rolling per-phase index lives
in [`PLAN.md`](PLAN.md), individual plan documents under
[`plans/`](plans/), and the per-version release notes in
[`CHANGELOG.md`](CHANGELOG.md).

For a high-level summary of what shipped when, the docs site has a
[Feature matrix page](https://nyankobu010.github.io/tako-ai-core/feature_matrix/)
([source](docs/feature_matrix.md)).

## Community

- Issues: <https://github.com/nyankobu010/tako-ai-core/issues>
- Discussions: <https://github.com/nyankobu010/tako-ai-core/discussions>
- Security: see [`SECURITY.md`](SECURITY.md) — please use GitHub *Private Vulnerability Reporting* rather than opening a public issue.
- Good first issues: <https://github.com/nyankobu010/tako-ai-core/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22>

## License

Apache-2.0 — see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
