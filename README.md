# tako 蛸

> **Rust-core, Python-facade framework for enterprise agentic systems.**
>
> Many arms, one mind.

[![CI](https://github.com/TODO(<org>)/tako-ai-core/actions/workflows/ci.yml/badge.svg)](https://github.com/TODO(<org>)/tako-ai-core/actions/workflows/ci.yml)
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

## Feature matrix

| Capability                         | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 | Phase 6 | Phase 7 | Phase 8 | Phase 9 | Phase 10 |
|------------------------------------|:-------:|:-------:|:-------:|:-------:|:-------:|:-------:|:-------:|:-------:|:-------:|:--------:|
| `LlmProvider` trait + adapters     | ✅ Anthropic, OpenAI, http-generic | ➕ Azure, Bedrock, Vertex | | ➕ Mistral, Ollama | | | | | | ➕ Python custom provider streaming |
| OpenAI-compat HTTP server          |         | ✅      |         |         |         |         |         | ➕ `tako.*` SSE extensions (Phase 9) | | ➕ `tako.tool_call_*` named events |
| MCP client (stdio + Streamable HTTP) | ✅    |         |         | ➕ WS, gRPC | ➕ gRPC mTLS |  |         |         |         | |
| `SingleAgent` orchestrator         | ✅      |         |         |         | ➕ budget |         |         |         |         | |
| `Conductor` orchestrator           |         | ✅      |         |         |         | ➕ budget |         |         |         | ➕ verifier scores |
| `Trinity` learned router           |         |         | ✅      |         |         | ➕ budget |         |         |         | ➕ verifier scores |
| `SelfCaller` recursion             |         |         | ✅      |         |         | ➕ judge budget | ✅ native streaming | ➕ streaming guard | | |
| `AbMcts` tree search               |         |         |         | ✅      |         |         |         | ✅ streaming + Python facade | ➕ router-driven branch expansion | |
| Streaming guards (`ConfidenceGuard::evaluate_streaming`) | | | | | | | | ✅ rule-based early-abort | ➕ opt-in `LlmJudgeGuard` per-N-delta | |
| OPA / Rego policy enforcement      |         | ✅      |         |         |         |         |         |         |         | |
| PII / DLP redaction                | ✅      |         |         |         |         |         |         |         |         | |
| OTel tracing (`tako.*`, `gen_ai.*`) | ✅     |         |         |         |         |         |         |         |         | |
| Budgets (in-memory)                | ✅      |         |         | ➕ Redis | ➕ SingleAgent wiring | ➕ Conductor / Trinity / Judge | | | | |
| Circuit breakers + rate limits     | ✅      |         |         |         |         |         |         |         |         | |
| Sigstore tool-catalogue verify     |         |         |         | ✅ keyed | ➕ keyless | ➕ chain + Rekor SET | ➕ Rekor inclusion proof + cosign protobuf bundle | ➕ Rekor checkpoint | ➕ checkpoint freshness anchor | ➕ on-disk `JsonStateStore` |
| Sync + async dual API              | ✅      |         |         |         |         |         |         |         |         | |

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

See [`PLAN.md`](PLAN.md) and [`ARCHITECTURE.md`](ARCHITECTURE.md) for details.

## Community

- Issues: <https://github.com/TODO(<org>)/tako-ai-core/issues>
- Discussions: TODO(community): set up GitHub Discussions categories Q&A / Ideas / Show and tell.
- Chat: TODO(community): create a Discord/Matrix room and link here.
- Good first issues: <https://github.com/TODO(<org>)/tako-ai-core/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22>

## License

Apache-2.0 — see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
