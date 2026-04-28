# tako 蛸

> Rust-core, Python-facade framework for enterprise agentic systems.

`tako` (Japanese: octopus — many arms, one mind) is an open-source framework for
building production agentic systems. It gives you vendor-neutral provider
abstractions, a Rust orchestration core that keeps Python's GIL out of the
hot path, MCP tool connectivity, and the governance plumbing — OTel
tracing, OPA policy, PII redaction, budgets, circuit breakers — you
actually need at scale, all under a Pythonic dual sync/async API that
ships as one `pip install`.

## Inspiration & credit

`tako` is an open-source generalisation of three patterns Sakana AI
published, plus AB-MCTS tree search:

- TRINITY — Xu et al., [arXiv:2512.04695](https://arxiv.org/abs/2512.04695)
- Conductor — Nielsen et al., [arXiv:2512.04388](https://arxiv.org/abs/2512.04388)
- Fugu Beta self-recursion — [Sakana AI blog](https://sakana.ai/fugu-beta/)
- AB-MCTS — Inoue et al., [arXiv:2503.04412](https://arxiv.org/abs/2503.04412)

`tako` is an independent open-source project. It is not affiliated with,
endorsed by, or sponsored by Sakana AI or any model provider.

## Phase 1 status

Phase 1 ships:

- `tako-core` traits + types
- `tako-runtime` budgets, breakers, rate limiters, retries
- Anthropic + OpenAI providers (chat, streaming, tool calls)
- `http-generic` template-driven provider for community contributions
- MCP stdio + Streamable HTTP transports + tool registry
- `SingleAgent` orchestrator
- OTel tracing pipeline + PII redaction + EnvResolver
- PyO3 wheel + Pydantic v2 facade + sync/async dual API

Phase 2-4 add Conductor, Trinity routing, AB-MCTS, OPA, Sigstore,
cloud-vendor providers and resolvers, and the OpenAI-compat server.
