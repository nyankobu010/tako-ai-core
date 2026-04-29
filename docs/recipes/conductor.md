# Recipe: Conductor

The `Conductor` orchestrator delegates work to a pool of worker
providers keyed by role name. A coordinator LLM emits structured
dispatch JSON; workers run in parallel under a configurable fanout cap.

## Minimal Conductor

```python
import tako

conductor = tako.Conductor(
    coordinator=tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."),
    workers={
        "code": tako.providers.OpenAI(model="gpt-5", api_key="..."),
        "math": tako.providers.Anthropic(model="claude-sonnet-4-6", api_key="..."),
    },
    max_fanout=4,
    max_steps=8,
    worker_timeout_secs=60,
    fail_fast=False,
)

result = await conductor.run("Implement merge sort and verify its O(n log n) bound.")
print(result.text)
```

## Coordinator output schema

The coordinator gets a system prompt containing this schema and is
expected to emit JSON matching it:

```json
{
  "workers": [
    {"name": "code", "task": "Implement merge sort in Python.", "tools": ["fs"]},
    {"name": "math", "task": "Verify the bound."}
  ],
  "join_strategy": "all",
  "next_step": "summarise"
}
```

`join_strategy` is `"all"` or `"any"`. `next_step` is `"summarise"` or
`"halt"`. Markdown ` ```json ` fences are stripped before parsing.
Malformed plans are fed back to the coordinator as a one-turn retry
(capped at `max_steps`).

## Knobs

| Knob | Default | What it does |
|------|---------|--------------|
| `max_steps` | `8` | Maximum coordinator+worker rounds |
| `max_fanout` | `4` | Concurrent workers per round (Semaphore-bounded) |
| `worker_timeout_secs` | `60` | Per-worker hard timeout |
| `fail_fast` | `false` | Abort on first worker error vs collect partial results |

## Cross-cloud

```python
conductor = tako.Conductor(
    coordinator=tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."),
    workers={
        "code": tako.providers.OpenAI(model="gpt-5", api_key="..."),
        "long": tako.providers.AzureOpenAI(
            endpoint=os.environ["AZURE_OPENAI_ENDPOINT"],
            deployment="gpt-4o-prod",
            api_key=os.environ["AZURE_OPENAI_API_KEY"],
        ),
        "math": tako.providers.Vertex(
            project_id="my-proj",
            model="gemini-2.0-pro",
            access_token=os.environ["VERTEX_ACCESS_TOKEN"],
        ),
    },
)
```

The provider ids show up as OTel span attributes
(`worker.provider.id`), so you can break down latency and cost by
worker role across vendors.
