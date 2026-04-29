# Tracing

`tako-governance::otel` wires `tracing` + `opentelemetry-otlp` so every
orchestrator run, provider call, and tool invocation emits structured
spans with both `tako.*` and `gen_ai.*` semconv attributes.

## Initialisation

```python
import tako

guard = tako.tracing.init_otlp(
    endpoint="http://otel-collector:4317",
    protocol="grpc",   # or "http"
    resource_attrs={"service.name": "my-agent", "deployment.env": "prod"},
)
# ... your application ...
tako.tracing.shutdown_otlp()  # flushes pending spans
```

`init_otlp` returns a process-global guard that flushes pending spans
on interpreter exit. You only call this once per process.

For local dev or tests, the lighter `tako.tracing.init()` writes to
stderr via `tracing-subscriber`.

## What gets emitted

| Span name | Notable attributes |
|-----------|-------------------|
| `tako.orchestrator.run` | `tako.orchestrator.kind`, `tako.principal.tenant_id`, `tako.principal.user_id` |
| `tako.orchestrator.dispatch` | `worker.name`, `worker.provider.id` (Conductor) |
| `tako.provider.chat` | `tako.provider.id`, `tako.provider.model`, `tako.tokens.input`, `tako.tokens.output`, `tako.cost.usd`, plus `gen_ai.system`, `gen_ai.request.model` |
| `tako.tool.invoke` | `tako.tool.name`, `tako.tool.duration_ms` |
| `tako.policy.evaluate` | `tako.policy.stage`, `tako.policy.decision` |

Spans are nested: orchestrator runs are the root; provider calls and
tool invocations are children. The Conductor's per-worker dispatches
are children of the orchestrator run.

## OTLP sanity check

```bash
docker run -d -p 4317:4317 otel/opentelemetry-collector-contrib
RUST_LOG=tako=debug python examples/01_single_agent.py
```

Open the collector's debug exporter or hook up Jaeger / Honeycomb /
Tempo / your collector of choice — any OTel backend works.
