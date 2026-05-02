# tako — architecture

`tako` is a Rust workspace + Python facade. The Rust core does the work; the
Python facade is a thin, ergonomic shell over a PyO3 extension module.

## Crate graph

```mermaid
graph TD
    core[tako-core<br/>traits, types, errors]

    runtime[tako-runtime<br/>budget, breaker, retry, limiter]
    runtime --> core

    providers[tako-providers/*<br/>anthropic, openai, azure-openai,<br/>bedrock, vertex, mistral, ollama,<br/>http-generic]
    providers --> core

    mcp[tako-mcp<br/>stdio, Streamable HTTP,<br/>WebSocket, gRPC mTLS]
    mcp --> core

    orch[tako-orchestrator<br/>SingleAgent, Conductor, Trinity,<br/>SelfCaller, AbMcts]
    orch --> core
    orch --> runtime

    gov[tako-governance<br/>OTel, OPA, PII, secrets,<br/>sigstore, StateStore]
    gov --> core

    compat[tako-compat<br/>OpenAI-compat HTTP server,<br/>AuthResolver impls]
    compat --> core
    compat --> orch

    py[tako-py<br/>PyO3 bindings]
    py --> core
    py --> runtime
    py --> providers
    py --> mcp
    py --> orch
    py --> gov
    py --> compat

    facade[python/tako<br/>Pydantic v2 facade]
    facade --> py
```

**Hard rules:**

- `tako-core` has no I/O, no Tokio. It defines the contracts — five public
  traits (`LlmProvider`, `Tool`, `McpTransport`, `Router`, `PolicyEngine`,
  `Verifier`) plus the request/response/error types every provider speaks.
- Provider crates depend only on `tako-core` + their vendor SDK + `reqwest` /
  `eventsource-stream`. They never depend on each other or on
  `tako-runtime`. Per-provider helpers (URL pre-fetch SSRF guard, MIME
  filters, data-URL prefix normalisation) are duplicated per crate rather
  than centralised — the dep-graph rule wins over DRY.
- `tako-py` is the only crate that knows about Python.
- `python/tako/` imports `tako._native` and **only** `tako._native`. End
  users `import tako.*`.

## Sequence: a `SingleAgent.run(prompt)` call

```mermaid
sequenceDiagram
    actor User
    participant Py as python/tako
    participant Native as tako._native (tako-py)
    participant Orch as SingleAgent
    participant Prov as LlmProvider impl
    participant Tools as ToolRegistry

    User->>Py: orch.run(prompt)
    Py->>Native: PyOrchestrator.run(prompt)
    Native->>Native: future_into_py(rt.spawn(...))
    Native->>Orch: orchestrator.run(principal, input)

    loop step <= max_steps
        Orch->>Prov: chat(principal, req)
        Prov-->>Orch: ChatResponse{content, tool_calls?}

        alt has tool_calls
            Orch->>Tools: invoke(name, args)
            Tools-->>Orch: ToolResult
            Orch->>Orch: append ToolResult to messages
        else final answer
            Orch-->>Native: OrchOutput{text, usage}
        end
    end

    Native-->>Py: PyOrchOutput
    Py-->>User: result.text, result.usage
```

OTel spans:

- root: `tako.orchestrator.run` with `tako.orchestrator.kind=single`,
  `tako.principal.tenant_id`, `tako.principal.user_id`
- per provider call: child `tako.provider.chat` with `tako.provider.id`,
  `tako.provider.model`, `tako.tokens.input`, `tako.tokens.output`,
  `tako.cost.usd`, plus the `gen_ai.*` semconv attributes
- per tool call: child `tako.tool.invoke` with `tako.tool.name`,
  `tako.tool.duration_ms`
- per policy decision: child `tako.policy.evaluate` with
  `tako.policy.stage`, `tako.policy.decision`

## Streaming sequence (`Conductor::stream`)

`Conductor`, `Trinity`, and `AbMcts` all stream natively via `OrchEvent`.
The wiring uses bounded `mpsc::channel(64)` channels for per-delta
backpressure: producers block on `send().await` once the consumer is
behind, capping in-flight memory under slow `evaluate_streaming`
verifiers or slow downstream sinks. Trinity is naturally serial (no
channel needed).

```mermaid
sequenceDiagram
    participant User
    participant Conductor
    participant Worker as Worker (mpsc producer)
    participant Verifier

    User->>Conductor: stream(prompt)
    par per worker (bounded fanout)
        Conductor->>Worker: provider.stream(...)
        loop per delta
            Worker-->>Conductor: ChatChunk::Delta
            Conductor-->>User: OrchEvent::AssistantText
            Conductor->>Verifier: evaluate_streaming(buf)
            Verifier-->>Conductor: Option<f32>
            Conductor-->>User: OrchEvent::VerifierScore { step, branch, score }
        end
        Worker-->>Conductor: ChatChunk::End
    end
    Conductor-->>User: synthesis-complete final
```

## Async + GIL discipline

`tako-py` builds a single shared Tokio runtime at module init via
`pyo3_async_runtimes::tokio::get_runtime()`. Every `#[pyfunction]` async
wrapper returns a Python awaitable produced by
`pyo3_async_runtimes::tokio::future_into_py`. Inside the future we **never**
hold the GIL across an `.await`. Sync siblings (`run_sync`) are wrapped in
`py.detach(|| runtime.block_on(...))`, releasing the GIL before blocking. The
`test_async_concurrency.py` suite runs 10 parallel orchestrator invocations
against a delaying `FakeProvider` and asserts wall-clock < 1.5× single-run
time — if the GIL leaks, this test fails.

## Data flow

```
ChatRequest (tako-core)
    ↓ to_vendor()
vendor JSON (per-provider)
    ↓ HTTPS / SSE
vendor response
    ↓ from_vendor()
ChatResponse / Stream<ChatChunk>
```

All vendor errors are mapped to `TakoError::Provider` with the original
status + body preserved in the structured `details` field. Streaming
contract: yield all received chunks, then `ChatChunk::Error`, then exactly
one `ChatChunk::End` — even on mid-stream failure.

Vision content (`ContentPart::Image`, `ContentPart::ImageUrl`) flows
through every SDK-backed provider; Bedrock and Ollama use opt-in
tako-side URL pre-fetch with full SSRF mitigation (default-on private-IP
blocklist + DNS-rebind defence + per-host / wildcard / CIDR allowlist).

## Reliability layers

```
Orchestrator (SingleAgent | Conductor | Trinity | SelfCaller | AbMcts)
    ↓
FallbackProvider (cascade)
    ↓
RateLimiter (governor)
    ↓
CircuitBreaker (failsafe)
    ↓
Retry with jitter (backoff)
    ↓
LlmProvider impl (vendor SDK / HTTP)
```

`BudgetTracker` is consulted both before the call (using
`LlmProvider::estimate_cost_usd`) and after (reconciling against actual
usage returned in the response). Budget backends ship in two flavours:
in-memory (single-process) and Redis (multi-replica, with monotonic-write
Lua so a slow replica cannot clobber a higher water-mark).

## Phase boundaries

The trait surface in `tako-core` is designed so each phase is purely
additive — public APIs from earlier phases never break. As of v0.35.0
(Phase 34), the project ships every capability described on this page.
For the chronological ledger of which capability landed in which phase,
see the [feature matrix in README.md](https://github.com/nyankobu010/tako-ai-core/blob/main/README.md#feature-matrix)
and [`PLAN.md`](https://github.com/nyankobu010/tako-ai-core/blob/main/PLAN.md).
