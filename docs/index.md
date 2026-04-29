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

## What's in v0.3.0 (Phase 2.5)

The current release adds **cloud breadth**:

- **Azure OpenAI** + **Vertex AI (Gemini)** providers (joining Anthropic,
  OpenAI, Bedrock, http-generic).
- Cloud secret resolvers: **Vault**, **AWS Secrets Manager**, **Azure Key
  Vault**, **GCP Secret Manager**.
- **Bedrock streaming** (ConverseStream) + **OpenAI-compat SSE** —
  closing the two `501 Not Implemented` surfaces left in v0.2.0.
- This **mkdocs site** with full nav + GitHub Pages deploy.

See [Architecture](architecture.md) for the crate graph + sequence
diagrams, [Concepts](concepts/providers.md) for the design surface, and
[Recipes](recipes/azure_openai.md) for end-to-end integration walkthroughs.

## Roadmap

| Phase | Status | Highlights |
|-------|--------|------------|
| 1 | ✅ v0.1.0 | Foundation: 3 providers, MCP, SingleAgent, OTel, wheel |
| 2 | ✅ v0.2.0 | Conductor, Bedrock, OPA, OpenAI-compat server |
| 2.5 | ✅ v0.3.0 | Azure + Vertex, cloud resolvers, Bedrock+SSE streaming, docs |
| 3 | next | Trinity learned routing, SelfCaller recursion, eval harness |
| 4 | future | AB-MCTS, Sigstore, Mistral / Ollama, Redis budgets |
