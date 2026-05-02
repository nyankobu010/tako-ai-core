# Streaming

Every orchestrator and every SDK-backed provider in `tako` streams
natively. This page covers the surface — `OrchEvent`, streaming guards,
streaming verifiers, and the bounded-channel backpressure design.

## OrchEvent

`Orchestrator::stream(...)` returns an `OrchEventStream` that yields
typed events:

```python
async for ev in orch.stream("Explain CRDTs"):
    if isinstance(ev, tako.OrchEvent.AssistantText):
        print(ev.text, end="", flush=True)
    elif isinstance(ev, tako.OrchEvent.VerifierScore):
        log_score(ev.step, ev.branch, ev.score)
    elif isinstance(ev, tako.OrchEvent.Recursion):
        log_recursion(ev.depth, ev.confidence)
    elif isinstance(ev, tako.OrchEvent.ToolCallStart):
        log_tool(ev.name, ev.args)
    elif isinstance(ev, tako.OrchEvent.ToolCallResult):
        log_tool_result(ev.name, ev.result)
```

The enum is `#[non_exhaustive]` on the Rust side so future variants do
not break match statements that handle the variants they care about and
fall through on the rest.

## Streaming guards (`ConfidenceGuard::evaluate_streaming`)

`SelfCaller::stream` evaluates a `ConfidenceGuard` on each cumulative
delta so it can short-circuit a clearly-converged answer mid-stream:

- **`RuleBasedGuard`** — cheap heuristic (length / regex). Off-the-shelf
  `tako.guards.RuleBased(min_chars=...)`.
- **`LlmJudgeGuard`** — opt-in per-N-delta judging:
  ```python
  guard = tako.guards.LlmJudge(judge=judge_provider, rubric="...")
  guard = guard.with_streaming_min_chars(80).with_streaming_every_n(50)
  ```
  The judge is only invoked once `min_chars` is reached and then every
  `N` deltas — keeps cost bounded.

## Streaming verifiers (`Verifier::evaluate_streaming`)

Trinity, Conductor, and AbMcts all drive `Verifier::evaluate_streaming`
on each cumulative delta and emit per-delta `OrchEvent::VerifierScore`
events with the same `(step, branch)` as the eventual
synthesis-complete final.

```rust
#[async_trait]
pub trait Verifier: Send + Sync + 'static {
    async fn evaluate(&self, principal: &Principal, output: &str) -> Result<f32, TakoError>;

    /// Default returns Ok(None). Override for per-delta scoring.
    async fn evaluate_streaming(
        &self,
        principal: &Principal,
        partial: &str,
    ) -> Result<Option<f32>, TakoError> {
        Ok(None)
    }
}
```

The shipped `RuleBasedVerifier` (and `tako.verifiers.RuleBased`)
overrides the hook out of the box; user-defined `Verifier`s can opt in.

## Branch identity

Streaming events from multi-worker orchestrators carry a `branch`
identifier so consumers can correlate partials with finals:

| Orchestrator | `branch` value |
|--------------|----------------|
| Trinity | role's positional index in the `roles` map |
| Conductor | 1-based dispatch index, stamped at task-construction time so it stays stable under concurrent worker completion |
| AbMcts | `leaf_idx as u32`, stamped before the leaf is pushed |

Partials and finals share `(step, branch)`. Consumers can either
de-duplicate on this pair or display partials live and replace with the
final on `OrchEvent::FinalText`.

## Backpressure

AbMcts and Conductor use bounded `mpsc::channel(64)` channels
internally for per-delta `OrchEvent` / `WorkerStreamEvent` flow.
Producers `await` on a full channel, so a slow consumer cannot blow up
in-flight memory under fast streaming workers. Trinity is naturally
serial (one stream at a time), so no channel is needed.

The 64-slot capacity matches the
[`tako-mcp/src/transport/grpc.rs`](https://github.com/nyankobu010/tako-ai-core/blob/main/crates/tako-mcp/src/transport/grpc.rs)
`NOTIFICATION_BUFFER` / `OUTBOUND_BUFFER` precedent — large enough to
amortise across normal jitter, small enough to bound memory under sustained
producer/consumer mismatch.

## See also

- [Orchestrators](orchestrators.md) — overview of `SingleAgent` /
  `Conductor` / `Trinity` / `SelfCaller` / `AbMcts`.
- [OpenAI-compat server](compat.md) — these `OrchEvent`s are re-emitted
  as `tako.*` SSE extension events.
