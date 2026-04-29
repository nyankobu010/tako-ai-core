# SelfCaller (bounded recursion)

Wrap any tako orchestrator in a `SelfCaller` to add corrective retries
when the output fails a confidence check.

## Rule-based guard

```python
import tako
from tako.guards import RuleBased

inner = tako.SingleAgent(
    provider=tako.providers.OpenAI(model="gpt-5", api_key="..."),
    max_steps=4,
)

sc = tako.SelfCaller(
    inner=inner,
    confidence=RuleBased(min_chars=120, pattern=r"\bbecause\b"),
    max_depth=2,
    min_confidence=0.5,
)

result = await sc.run("Why is CRDTs convergence guaranteed?")
print(result.text)
```

`RuleBased` returns `1.0` when the output is at least `min_chars` long
AND matches the optional regex; `0.0` otherwise. Cheap and
deterministic; great for "is this answer well-formed?" gates.

## LLM-as-judge

```python
from tako.guards import LlmJudge

judge = tako.providers.Anthropic(model="claude-opus-4-7", api_key="...")
sc = tako.SelfCaller(
    inner=inner,
    confidence=LlmJudge(judge, "Score the answer's correctness from 0 to 1."),
    max_depth=2,
    min_confidence=0.7,
)
```

The judge is asked to reply with ONLY a decimal between 0 and 1.
Anything unparseable falls back to `0.5`.

## Termination

`SelfCaller` runs the inner orchestrator at most `max_depth + 1`
times. Even with an adversarial guard that always reports `0.0`, the
loop terminates within that bound and returns the last output.

## Depth across nested wrappers

Depth is stored as a string in `Principal.metadata["tako.recursion.depth"]`,
so nested SelfCallers across module boundaries see the SAME counter.

```python
inner = tako.SelfCaller(inner=tako.SingleAgent(...), confidence=guard_a, max_depth=2)
outer = tako.SelfCaller(inner=inner, confidence=guard_b, max_depth=2)
# outer recursion + inner recursion bounded by the same depth metadata.
```

## Composing with Trinity

A common pattern: route per-step (Trinity), then bound retries
(SelfCaller).

```python
trinity = tako.Trinity(roles={...}, router=...)
sc = tako.SelfCaller(inner=trinity, confidence=LlmJudge(...), max_depth=2)
```
