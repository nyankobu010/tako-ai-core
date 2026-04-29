# Rust API reference

The Rust API is documented via standard rustdoc. Until we publish to
crates.io, browse it locally:

```bash
cargo doc --workspace --no-deps --open
```

Or after the first crates.io publish, on docs.rs:

| Crate | docs.rs |
|-------|---------|
| `tako-core` | [docs.rs/tako-core](https://docs.rs/tako-core) |
| `tako-runtime` | [docs.rs/tako-runtime](https://docs.rs/tako-runtime) |
| `tako-orchestrator` | [docs.rs/tako-orchestrator](https://docs.rs/tako-orchestrator) |
| `tako-governance` | [docs.rs/tako-governance](https://docs.rs/tako-governance) |
| `tako-mcp` | [docs.rs/tako-mcp](https://docs.rs/tako-mcp) |
| `tako-compat` | [docs.rs/tako-compat](https://docs.rs/tako-compat) |
| `tako-providers-anthropic` | [docs.rs/tako-providers-anthropic](https://docs.rs/tako-providers-anthropic) |
| `tako-providers-openai` | [docs.rs/tako-providers-openai](https://docs.rs/tako-providers-openai) |
| `tako-providers-azure-openai` | [docs.rs/tako-providers-azure-openai](https://docs.rs/tako-providers-azure-openai) |
| `tako-providers-bedrock` | [docs.rs/tako-providers-bedrock](https://docs.rs/tako-providers-bedrock) |
| `tako-providers-vertex` | [docs.rs/tako-providers-vertex](https://docs.rs/tako-providers-vertex) |
| `tako-providers-http-generic` | [docs.rs/tako-providers-http-generic](https://docs.rs/tako-providers-http-generic) |

## Core surface

The five public traits in `tako-core`:

- `LlmProvider` — vendor-neutral chat + stream API
- `Tool` — single tool invocation
- `McpTransport` — MCP client transport (stdio, HTTP, …)
- `Router` — provider/role selection (Phase 3 Trinity)
- `PolicyEngine` — Allow / Deny / RedactMessages / ForceModel decisions

Public types:

- `Capabilities`, `ChatRequest`, `ChatResponse`, `ChatChunk`,
  `ContentPart`, `FinishReason`, `Message`, `Principal`, `Role`,
  `ToolCallDelta`, `ToolSchema`, `Usage`
- `TakoError` (errors are flat — match on the variant)
