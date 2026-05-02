# Providers

`tako` exposes one trait — `LlmProvider` — that every model adapter
implements. Provider crates depend only on `tako-core` plus their vendor
SDK; they never depend on each other or on `tako-runtime`. This keeps
the dependency graph shallow and makes adding a new provider a
mechanical exercise.

## Available providers

| Provider | Crate | Auth | Streaming | Tool calls | Vision |
|----------|-------|------|-----------|------------|--------|
| OpenAI | `tako-providers-openai` | API key | ✅ SSE | ✅ | ✅ inline + URL |
| Anthropic | `tako-providers-anthropic` | API key | ✅ SSE | ✅ | ✅ inline + URL |
| Azure OpenAI | `tako-providers-azure-openai` | API key | ✅ SSE | ✅ | ✅ inline + URL |
| AWS Bedrock | `tako-providers-bedrock` | AWS chain | ✅ ConverseStream | ✅ | ✅ inline + URL via tako pre-fetch |
| Vertex AI (Gemini) | `tako-providers-vertex` | OAuth2 token | ✅ SSE | ✅ | ✅ inline + `gs://` / `https://` `fileData` |
| Mistral | `tako-providers-mistral` | API key | ✅ SSE | ✅ | ✅ inline + URL |
| Ollama | `tako-providers-ollama` | local socket | ✅ NDJSON | ✅ | ✅ inline + URL via tako pre-fetch |
| HTTP-generic | `tako-providers-http-generic` | template-driven | ⚠️ via `StreamConfig` | ⚠️ via template | — |
| `PythonProvider` | `tako-py` | n/a | ✅ async-gen | ✅ | inline only |

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

1. **`http-generic`** — point a template at any HTTP endpoint that speaks
   JSON in/out. Streaming via `StreamConfig::OpenAiSse` or
   `StreamConfig::NdJson` with JSON-pointer-based delta extraction.
   Python facade: `tako.providers.HttpGeneric(...)`.
2. **`PythonProvider`** — implement `chat()` (and optionally a streaming
   `async_gen`) as Python callables. Useful for prototyping or adapting
   to non-HTTP transports.

## Capabilities

Each provider exposes a `Capabilities` struct — `max_context_tokens`,
`supports_streaming`, `supports_tools`, `supports_vision`,
`supports_json_mode`, plus optional per-million-token cost. The
orchestrator consults capabilities before dispatching (e.g. it will fall
through to `provider.chat(...)` for a worker whose
`supports_streaming` is false).

## Vision content

All seven SDK-backed providers handle outbound vision content:

```python
from tako import ChatRequest, ContentPart, Message, Role

req = ChatRequest(messages=[
    Message(role=Role.User, content=[
        ContentPart.text("Describe this image."),
        ContentPart.image(mime="image/jpeg", data_b64=jpeg_bytes_base64),
        # or, on every provider whose vendor server fetches URLs:
        ContentPart.image_url(url="https://example.com/photo.jpg"),
    ]),
])
```

For Bedrock + Ollama, URL-source images are fetched server-side by
*tako* (the vendor wire formats don't accept URLs); see [URL
pre-fetch](url_prefetch.md) for the SSRF mitigation stack.

## See also

- [Vision content](vision.md) — inline + URL-source images, per-provider wire shapes.
- [URL pre-fetch & SSRF](url_prefetch.md) — opt-in tako-side URL fetcher
  with private-IP blocklist + DNS-rebind defence + allowlist override.
- [recipes/](../recipes/azure_openai.md) — end-to-end integration walkthroughs.
