# SelfCaller (bounded recursion)

`SelfCaller` is a wrapper that runs an inner orchestrator, scores the
output with a `ConfidenceGuard`, and if the score is too low (and
recursion depth hasn't hit the cap), feeds the output back through the
inner orchestrator with a revision prompt appended.

This is `tako`'s implementation of the Sakana *Fugu Beta* self-recursive
test-time scaling pattern.

## Anatomy

```python
sc = tako.SelfCaller(
    inner=tako.SingleAgent(provider=...),
    confidence=tako.guards.RuleBased(min_chars=80),
    max_depth=3,
    min_confidence=0.7,
    revision_prompt=None,  # uses the default if None
)
```

- `inner`: any orchestrator (`SingleAgent`, `Conductor`, `Trinity`).
  Trait-object form so heterogeneous wrappers compose.
- `confidence`: a `ConfidenceGuard`. Built-ins:
    - `tako.guards.RuleBased(min_chars=..., pattern=...)` — cheap rule.
    - `tako.guards.LlmJudge(judge=..., rubric=...)` — LLM-as-judge.
- `max_depth`: hard cap on recursion. Default `3`. The inner
  orchestrator is invoked at most `max_depth + 1` times.
- `min_confidence`: threshold in `[0, 1]`. Confidence ≥ threshold ⇒ stop.
- `revision_prompt`: the user message appended to the conversation when
  recursing. The default points the model at "your previous answer
  scored low; correct it."

## Termination

`SelfCaller` is guaranteed to terminate within `max_depth + 1` inner
runs even when the guard never reports confidence. The
`tests/python/test_self_caller.py::test_terminates_within_max_depth_on_adversarial`
test pins this for adversarial guards.

## Depth tracking

Depth is stored as a string in `Principal.metadata` under the key
`tako.recursion.depth`. Each recursion increments the value; this means
nested SelfCallers in different orchestrator stacks share a single
depth counter, so accidental loops across module boundaries are
impossible.

## OTel

Each inner invocation emits a `tako.recursion.step` span with
`tako.recursion.depth` and `tako.recursion.confidence` attributes.

## When to use

| Situation | Use |
|-----------|-----|
| Mechanical pass criterion (length, regex) | `RuleBased` |
| Judgmental pass criterion ("is this a good answer?") | `LlmJudge` with a stronger judge model |
| Need to combine routing + recursion | wrap a `Trinity` in a `SelfCaller` |
| Cost-bounded; can't afford retries | use `max_depth=1` or skip SelfCaller |

## When *not* to use

- The inner orchestrator already has a tool-call loop and the failure
  mode is "tool returned an error". Better fixes: improve the tool
  schema, raise the SingleAgent `max_steps`.
- Latency budget is tight; bounded recursion linearly multiplies
  worst-case time-to-first-byte.
