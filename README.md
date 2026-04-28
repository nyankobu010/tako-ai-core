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

| Capability                         | Phase 1 | Phase 2 | Phase 3 | Phase 4 |
|------------------------------------|:-------:|:-------:|:-------:|:-------:|
| `LlmProvider` trait + adapters     | ✅ Anthropic, OpenAI, http-generic | ➕ Azure, Bedrock, Vertex | | ➕ Mistral, Ollama |
| OpenAI-compat HTTP server          |         | ✅      |         |         |
| MCP client (stdio + Streamable HTTP) | ✅    |         |         | ➕ WS, gRPC |
| `SingleAgent` orchestrator         | ✅      |         |         |         |
| `Conductor` orchestrator           |         | ✅      |         |         |
| `Trinity` learned router           |         |         | ✅      |         |
| `SelfCaller` recursion             |         |         | ✅      |         |
| `AbMcts` tree search               |         |         |         | ✅      |
| OPA / Rego policy enforcement      |         | ✅      |         |         |
| PII / DLP redaction                | ✅      |         |         |         |
| OTel tracing (`tako.*`, `gen_ai.*`) | ✅     |         |         |         |
| Budgets (in-memory)                | ✅      |         |         | ➕ Redis |
| Circuit breakers + rate limits     | ✅      |         |         |         |
| Sigstore tool-catalogue verify     |         |         |         | ✅      |
| Sync + async dual API              | ✅      |         |         |         |

## Roadmap

- **Phase 1 — Foundation** *(in progress)*: traits, runtime, two providers,
  MCP basics, `SingleAgent`, OTel, PyO3 wheel, CI green.
- **Phase 2 — Orchestration**: `Conductor`, OPA enforcement, OpenAI-compat
  server, Bedrock / Vertex / Azure providers, Vault / cloud secrets.
- **Phase 3 — Learned coordination**: `Trinity` router with ONNX, training
  harness, `SelfCaller` recursion, eval harness.
- **Phase 4 — Search & scale**: AB-MCTS with verifiers, Mistral / Ollama,
  WebSocket / gRPC MCP, Sigstore verification, Redis budget backend.

See [`PLAN.md`](PLAN.md) and [`ARCHITECTURE.md`](ARCHITECTURE.md) for details.

## Community

- Issues: <https://github.com/TODO(<org>)/tako-ai-core/issues>
- Discussions: TODO(community): set up GitHub Discussions categories Q&A / Ideas / Show and tell.
- Chat: TODO(community): create a Discord/Matrix room and link here.
- Good first issues: <https://github.com/TODO(<org>)/tako-ai-core/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22>

## License

Apache-2.0 — see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
