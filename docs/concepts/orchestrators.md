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

## Trinity

A small `Router` (rule-based or ONNX-backed) selects which provider
handles each turn, so per-step model choice is data-driven instead of
coordinator-driven (Conductor) or static (SingleAgent).

```python
from tako.routers import RegexRouter

trinity = tako.Trinity(
    roles={
        "code": tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."),
        "math": tako.providers.OpenAI(model="gpt-5", api_key="..."),
        "fallback": tako.providers.OpenAI(model="gpt-5-mini", api_key="..."),
    },
    router=RegexRouter(),
)
result = await trinity.run("Solve x^2 + 5x + 6 = 0")
```

Use `Trinity` when:

- You have models with different strengths and want them picked per
  prompt without a coordinator round-trip.
- You can train a small classifier on past rollouts (see
  `tako.training.trinity.TrinityTrainer`) and load the result via
  `tako.routers.OnnxRouter`.

## SelfCaller

Bounded-recursion wrapper around any other orchestrator. After each
inner run, a `ConfidenceGuard` scores the output on `[0, 1]`. If the
score is below the threshold AND recursion depth is below `max_depth`,
the agent reads its previous output and produces a revision.

```python
from tako.guards import RuleBased

inner = tako.SingleAgent(provider=tako.providers.Anthropic(...))
sc = tako.SelfCaller(
    inner=inner,
    confidence=RuleBased(min_chars=80),
    max_depth=3,
    min_confidence=0.7,
)
result = await sc.run("Explain CRDTs")
```

Depth is tracked in `Principal.metadata["tako.recursion.depth"]` so
nested SelfCallers across module boundaries respect the same cap;
accidental infinite loops are impossible.

Use `SelfCaller` when:

- The acceptance criterion for an answer is mechanical (length, regex,
  unit-test-style verifier) — wrap with `RuleBased`.
- The acceptance criterion is judgmental — wrap with `LlmJudge` and
  point it at a stronger model than the inner orchestrator uses.

## Phase-4 orchestrators (preview)

- **AbMcts**: Adaptive Branching Monte Carlo Tree Search with
  pluggable verifiers; chooses both refine-vs-generate and which
  model to use at each tree node.
