# Routing

A `Router` picks one provider id from a candidate list. It's the
contract `Trinity` uses for per-step model selection, and the
`SingleAgent` opt-in shape for cross-provider routing without
role-switching.

The trait lives in `tako-core`:

```rust
#[async_trait]
pub trait Router: Send + Sync + 'static {
    async fn route(
        &self,
        principal: &Principal,
        req: &ChatRequest,
        candidates: &[String],
    ) -> Result<RoutingDecision, TakoError>;
}
```

`RoutingDecision` carries the chosen provider id, a confidence in
`[0, 1]`, and an optional reason string surfaced to OTel.

## RegexRouter (rule-based default)

The cheap default. A featuriser (`tako_orchestrator::features::featurise_text`)
extracts a 16-dim feature vector from the most recent user message;
hand-tuned rules map features to a candidate index.

Built-in defaults assume the candidate list is ordered
`[code, math, fallback]`, but you can build a custom rule chain via
`RegexRouter::builder()`.

```python
from tako.routers import RegexRouter
router = RegexRouter()
```

The featuriser is shared with the Python training harness
(`tako.training.features.featurise_text`); the parity test in
`tests/python/test_features_parity.py` asserts byte-identical outputs.

## OnnxRouter (learned)

A 2-layer MLP classifier loaded from an ONNX file. Available behind the
`onnx` Cargo feature (off by default to keep wheels slim and avoid
linking `libonnxruntime` unless you need it).

```python
from tako.routers import OnnxRouter
router = OnnxRouter("/path/to/trinity-router.onnx")
```

Train one in Python:

```python
from tako.training.trinity import TrinityTrainer

(
    TrinityTrainer(epochs=300, lr=0.05)
    .fit_jsonl("rollouts.jsonl")
    .export_onnx("trinity-router.onnx")
)
```

The trainer expects a JSONL file of
`{"prompt": "...", "label": <candidate-index>}` rows. Generate the
labels by running each prompt against every candidate provider,
scoring with a verifier (LLM-as-judge by default), and taking
argmax. See [`recipes/trinity.md`](../recipes/trinity.md) for an
end-to-end walkthrough.

## OTel

Both routers emit a `tako.router.route` span with attributes
`tako.router.kind` (`"regex"` or `"onnx"`), `tako.router.choice`
(provider id), and `tako.router.confidence` (decimal).

## Composing with SingleAgent

Pass `router=` along with `candidates=[...]` to enable per-step
selection over `[provider, *candidates]`:

```python
agent = tako.SingleAgent(
    provider=primary,
    candidates=[fast_model, big_model],
    router=RegexRouter(),
)
```

When `router` is None, the primary provider is used unconditionally —
the API stays backward-compatible.
