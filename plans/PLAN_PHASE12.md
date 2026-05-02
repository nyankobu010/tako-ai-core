# PLAN_PHASE12.md — Phase 12

> Closed-out plan. Phase 12 shipped on 2026-04-30 as v0.13.0.

Phase 12 paid down two long-standing debts surfaced in the Phase 11
close-out [PLAN.md](PLAN.md):

- **A. MCP Streamable HTTP — SSE notifications + `Mcp-Session-Id`
  lifecycle.** Promised in Phase 2; transport had been yielding an
  empty stream from `notifications()` ever since.
- **B. `tako.providers.HttpGeneric` Python facade.** The Rust
  `HttpGenericProvider` shipped in Phase 11.B with chat + streaming
  via `StreamConfig`, but had no PyO3 binding — Python users had to
  drop down to Rust to construct one.

Neither item required new architectural decisions; both followed
existing patterns (WebSocket transport for SSE, `PyBedrock` for the
provider facade), so they were bundled as one phase.

## A. MCP SSE notifications

### What shipped

`crates/tako-mcp/src/transport/streamable_http.rs:153-156` previously
returned `futures::stream::empty()`. Replaced with a lazy SSE reader,
following the `WebSocketTransport` blueprint:

- **Long-lived `GET {url}`** with `Accept: text/event-stream`, opened
  on the first call to `notifications()`. A guarding
  `AtomicBool::compare_exchange` ensures concurrent calls share one
  upstream connection.
- **`eventsource_stream::Eventsource`** parses each `data:` line as
  JSON-RPC. Method-bearing frames (no `id`) are broadcast to every
  subscriber via `tokio::sync::broadcast`. Frames carrying an `id` are
  silently dropped — those are POST responses delivered inline by
  `request()`, never on the GET stream.
- **`Mcp-Session-Id` propagation.** The transport already captured the
  session id from POST responses; the SSE GET now reads the latched
  value and attaches the header so servers that scope subscriptions
  per session see the correct id.
- **`close()`** signals the reader via a `tokio::sync::Notify`.

### Tests

`crates/tako-mcp/tests/streamable_http_sse.rs` — four wiremock-driven
integration tests:

1. Notification fan-out preserves order.
2. Two `notifications()` subscribers share one upstream GET
   (`expect(1)`).
3. Frames carrying an `id` are dropped (not delivered as
   notifications).
4. `Mcp-Session-Id` from a prior POST is attached to the SSE GET
   header.

## B. `tako.providers.HttpGeneric` Python facade

### What shipped

`crates/tako-py/src/py_http_generic.rs` (new, ~110 lines) — mirrors
the `PyBedrock` pattern. A `#[pyclass(name = "HttpGeneric")]` whose
`#[new]` takes the same fields as `HttpGenericConfig`. Marshalling:

- `body_template` and `stream_config` arrive as Python values
  (dict / list / scalar) and convert to `serde_json::Value` via
  `crate::conv::py_to_json`.
- `StreamConfig` deserialises directly from the converted value
  thanks to its existing `#[serde(tag = "kind", rename_all =
  "snake_case")]` attribute. No enum-mapping plumbing in PyO3.
- Construction is synchronous (the upstream builder doesn't `.await`),
  so unlike `PyBedrock` no `block_on` or GIL detach is needed.
- `supports_streaming()` reads `Capabilities::supports_streaming` so
  Python can probe the streaming-capability bit without dropping into
  Rust.

`PyHttpGeneric` wired into both provider-extraction sites:
- The inline if-else chain in
  `crates/tako-py/src/py_orchestrator.rs::Orchestrator::new`.
- The central `extract_provider` helper in
  `crates/tako-py/src/py_conductor.rs` (used by Conductor / Trinity /
  AB-MCTS / SelfCaller via `extract_any_provider`).

`python/tako/providers.py` adds an `HttpGeneric(_ProviderBase)`
wrapper. `python/tako/_native.pyi` adds the matching stub.

### Tests

`tests/python/test_http_generic_provider.py` — six tests:

1. Construction with required args succeeds; `id` matches.
2. `stream_config={"kind": "openai_sse"}` flips `supports_streaming`.
3. `stream_config={"kind": "ndjson", ...}` flips `supports_streaming`.
4. Empty `id` raises `ValueError` (Rust validator surfaces).
5. Unknown `stream_config` kind raises `ValueError` (serde dispatch).
6. `SingleAgent(provider=HttpGeneric(...))` constructs (proves
   provider-extraction chain accepts the new class).

## Out of scope (deferred to Phase 13 candidates)

Carry-forward from PLAN.md:

- Vision / image content support across providers
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond)
- Redis-backed `StateStore`
- Streaming-aware verifier in Trinity / Conductor
- `tako-compat` real auth providers (Vault / JWT / OIDC)
