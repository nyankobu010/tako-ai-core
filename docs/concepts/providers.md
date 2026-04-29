# Providers

`tako` exposes one trait — `LlmProvider` — that every model adapter
implements. Provider crates depend only on `tako-core` plus their vendor
SDK; they never depend on each other or on `tako-runtime`. This keeps
the dependency graph shallow and makes adding a new provider a
mechanical exercise.

## Available providers

| Provider | Crate | Auth | Streaming | Tool calls |
|----------|-------|------|-----------|-----------|
| OpenAI | `tako-providers-openai` | API key | ✅ SSE | ✅ |
| Anthropic | `tako-providers-anthropic` | API key | ✅ SSE | ✅ |
| Azure OpenAI | `tako-providers-azure-openai` | API key | ✅ SSE | ✅ |
| Bedrock | `tako-providers-bedrock` | AWS chain | ✅ ConverseStream | ✅ |
| Vertex AI (Gemini) | `tako-providers-vertex` | OAuth2 token (deferred) | ✅ SSE | ✅ |
| HTTP-generic | `tako-providers-http-generic` | template-driven | ⚠️ via template | ⚠️ via template |

## Choosing a provider

For most users the question is "which API surface do I already have
credentials for". The tako trait surface is identical across providers,
so you can A/B them with a one-line swap.

```python
provider = tako.providers.OpenAI(model="gpt-5", api_key="...")
# vs
provider = tako.providers.AzureOpenAI(
    endpoint="https://my-resource.openai.azure.com",
    deployment="gpt-4o-prod",
    api_key="...",
)
```

`Conductor` orchestration takes a *map* of providers keyed by worker
role, so a single deployment can route `code` workers to Anthropic and
`math` workers to Vertex without forking the orchestrator.

## Custom providers

If you need a provider that doesn't ship in the workspace (e.g. an
internal LLM gateway), you have two options:

1. **`http-generic`** — point a YAML template at any HTTP endpoint that
   speaks JSON in/out.
2. **`PythonProvider`** — implement `chat()` as an async Python
   callable. Useful for prototyping or adapting to non-HTTP transports.
   See [recipes](../recipes/azure_openai.md) for full examples.

## Capabilities

Each provider exposes a `Capabilities` struct — `max_context_tokens`,
`supports_streaming`, `supports_tools`, `supports_vision`,
`supports_json_mode`, plus optional per-million-token cost. The
orchestrator consults capabilities before dispatching (e.g. it will
refuse to send a streaming request to a provider that returns
`supports_streaming: false`).
