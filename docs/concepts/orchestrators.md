# Orchestrators

An *orchestrator* drives the agent loop: send a request to the provider,
optionally invoke tools, feed results back, repeat until done.

`tako` ships two orchestrators today; later phases add learned routing
and tree search.

## SingleAgent

The default. One provider, one tool registry, a max-step loop:

```python
agent = tako.SingleAgent(
    provider=tako.providers.OpenAI(model="gpt-5", api_key="..."),
    max_steps=10,
)
result = await agent.run("Find the weather in Tokyo and summarize.")
```

`SingleAgent` is the right choice when:

- You have a single capable model that handles the whole conversation.
- Tool dispatch is straightforward (no per-task model selection).
- You want minimal latency and deterministic cost behavior.

## Conductor

A coordinator-LLM emits structured dispatch JSON; workers (provider
adapters keyed by role name) run in parallel under a configurable
fanout cap.

```python
conductor = tako.Conductor(
    coordinator=tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."),
    workers={
        "code": tako.providers.OpenAI(model="gpt-5", api_key="..."),
        "math": tako.providers.Anthropic(model="claude-sonnet-4-6", api_key="..."),
    },
    max_fanout=4,
    worker_timeout_secs=60,
    fail_fast=False,
)
result = await conductor.run("Implement merge sort and verify its bound.")
```

The coordinator emits this JSON shape (handed to it via system prompt):

```json
{
  "workers": [
    {"name": "code", "task": "Implement merge sort in Python.", "tools": ["fs"]},
    {"name": "math", "task": "Verify the O(n log n) bound."}
  ],
  "join_strategy": "all",
  "next_step": "summarise"
}
```

Use `Conductor` when:

- Different sub-tasks need different model strengths.
- You can amortize coordinator latency across a wider fanout.
- Failure isolation matters (`fail_fast: false` returns partial results).

## Phase-3 orchestrators (preview)

- **Trinity**: a small learned router (rule + ONNX) selects the
  provider per step instead of having a coordinator emit JSON.
- **SelfCaller**: bounded recursion — an agent reads its own output and
  decides whether to spawn a corrective sub-agent.
- **AbMcts**: Adaptive Branching MCTS with verifiers.
