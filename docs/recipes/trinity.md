# Trinity (rule + ONNX routing)

Trinity routes each turn to one of N candidate providers via a Router
trait impl. This recipe walks through the rule-based and ONNX paths
end-to-end.

## Rule-based router (no training data)

```python
import tako
from tako.routers import RegexRouter

trinity = tako.Trinity(
    roles={
        "code":     tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."),
        "math":     tako.providers.OpenAI(model="gpt-5",            api_key="..."),
        "fallback": tako.providers.OpenAI(model="gpt-5-mini",       api_key="..."),
    },
    router=RegexRouter(),
)

print((await trinity.run("Write a Rust fn to compute fib")).text)   # → routed to "code"
print((await trinity.run("Solve 2 + 2")).text)                       # → routed to "math"
print((await trinity.run("hello there")).text)                       # → routed to "fallback"
```

`RegexRouter()` ships with three default rules over the shared
featuriser; build a custom rule chain via `RegexRouter::builder()` in
Rust if you want different splits.

## ONNX router (trained)

Step 1 — generate rollouts. For each prompt in your dataset, run all
candidates and score with a verifier model. The "winning" candidate is
the label.

Step 2 — train and export.

```python
from tako.training.trinity import Rollout, TrinityTrainer

rolls = [
    Rollout.from_prompt("write a fn", label=0),
    Rollout.from_prompt("solve x+1=2", label=1),
    Rollout.from_prompt("hi friend", label=2),
    # ... 100s of these
]
TrinityTrainer(epochs=300, lr=0.05).fit(rolls).export_onnx("trinity.onnx")
```

Or via the CLI:

```bash
python -m tako.training.trinity --rollouts rollouts.jsonl --out trinity.onnx
```

The exported model takes a `float32[1, 16]` input named `features` and
emits a `float32[1, K]` output named `logits`. The Rust featuriser
matches the Python featuriser byte-for-byte (asserted by
`tests/python/test_features_parity.py`), so inference at runtime hits
the same input distribution the model was trained on.

Step 3 — wire it up.

```python
from tako.routers import OnnxRouter

trinity = tako.Trinity(
    roles={
        "code":     tako.providers.Anthropic(...),
        "math":     tako.providers.OpenAI(...),
        "fallback": tako.providers.OpenAI(...),
    },
    router=OnnxRouter("trinity.onnx"),
)
```

`OnnxRouter` is gated behind the `onnx` Cargo feature. Build a wheel
that includes it with `maturin build --features onnx`. The native
ONNX runtime library (`libonnxruntime.{so,dylib,dll}`) must be
available on the dynamic-loader search path; we use `ort`'s
`load-dynamic` mode so the wheel itself stays slim.

## Caveat: candidate order matters

The router picks an index; `Trinity` maps that index back to a role
name in insertion order. Your training labels MUST line up with the
order you pass to `Trinity(roles={...})`. Use ordered dicts (Python
preserves insertion order on `dict` since 3.7) or pass explicit lists.
