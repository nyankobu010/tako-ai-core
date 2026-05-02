# PLAN — Phase 48 (Stable Vertex tool-call IDs in streaming)

> **Status: in progress.** Targets v0.49.0. Carry-forward
> from [Phase 46](PLAN_PHASE46.md) — closes the streaming-side
> ID stability gap that 46.C explicitly left out of scope.

## Context

Phase 46.C (v0.47.0) replaced the position-derived
`vertex_call_<n>` placeholder ID in **non-streaming**
Vertex responses with a stable
`SipHash13((name, args-as-canonical-JSON))` digest in
[`from_vertex_response`](../crates/tako-providers/vertex/src/convert.rs#L375-L398).

The **streaming** path
([`crates/tako-providers/vertex/src/stream.rs:67-75`](../crates/tako-providers/vertex/src/stream.rs#L67-L75))
still emits position-derived IDs:

```rust
if let Some(fc) = part.function_call {
    tool_calls.push(ToolCallDelta {
        index: tool_call_index,
        id: Some(format!("vertex_call_{tool_call_index}")),
        name: Some(fc.name),
        arguments_fragment: Some(fc.args.to_string()),
    });
    tool_call_index += 1;
}
```

Phase 46.C explicitly left this for a follow-up:

> Streaming path (`stream.rs`) still uses per-stream
> `tool_call_index` — that's a within-stream chunk
> reassembly concern, different from the cross-call
> identity concern this phase fixes. Out of scope until ask.

Looking at the data flow more carefully, the assumption
behind that deferral was wrong: **Vertex's SSE actually
emits one complete `function_call` per delta** — `fc.args`
is the full args object, not a fragment. The streaming
path is structurally similar to `from_vertex_response`:
each `function_call` we see has both `name` and full
`args`, so the same hash technique applies directly.

That means the gap is real today: the same logical tool
call gets different IDs from `chat()` vs `stream()`
against the same Vertex endpoint. Operators correlating
a streamed call with a non-streamed retry (a common
failover pattern) can't dedupe on `id`.

## Why now

Phase 46.C closed half the door; this phase closes the
other half. The fix is mechanical:

1. Factor the hash into a shared helper in
   `convert.rs`.
2. Call it from both
   [`from_vertex_response`](../crates/tako-providers/vertex/src/convert.rs#L375-L398)
   and
   [`into_chat_stream`](../crates/tako-providers/vertex/src/stream.rs#L67-L75).
3. Add a streaming test that asserts:
   - The streamed ID matches what
     `from_vertex_response` would emit for the
     equivalent non-streamed payload.
   - Multiple distinct calls in one stream get distinct
     IDs.

`tool_call_index` stays for within-stream chunk
reassembly — same `ToolCallDelta::index` field, just no
longer doing double duty as the consumer-visible ID.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 48.A | Extract `vertex_tool_call_id(name, args)` helper, make `pub(crate)` | [`crates/tako-providers/vertex/src/convert.rs`](../crates/tako-providers/vertex/src/convert.rs) |
| 48.B | Call the helper from `into_chat_stream` instead of position-derived format | [`crates/tako-providers/vertex/src/stream.rs`](../crates/tako-providers/vertex/src/stream.rs) |
| 48.C | New stream test: ID stable across runs + matches non-streaming for same payload | [`crates/tako-providers/vertex/tests/chat.rs`](../crates/tako-providers/vertex/tests/chat.rs) |
| 48.D | Workspace + Python version 0.48.0 → 0.49.0 | various |
| 48.E | PLAN.md row | [`PLAN.md`](../PLAN.md) |
| 48.F | CHANGELOG.md `[0.49.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 48.A — Shared helper

In [`convert.rs`](../crates/tako-providers/vertex/src/convert.rs):

```rust
/// Phase 46.C / 48 — stable tool-call ID for Vertex
/// responses. Hashes `(name, args-as-JSON)` so the same
/// logical call gets the same ID across:
///   - re-fetches of the same non-streaming response,
///   - re-streams of the same streaming response,
///   - non-streaming vs streaming of the same response.
///
/// `DefaultHasher` is `SipHash13` — non-cryptographic but
/// deterministic per-process for a given input. Suffix is
/// the lowercase 16-hex-char digest.
pub(crate) fn vertex_tool_call_id(name: &str, args: &serde_json::Value) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    serde_json::to_string(args).unwrap_or_default().hash(&mut hasher);
    format!("vertex_call_{:016x}", hasher.finish())
}
```

The non-streaming caller in `from_vertex_response`
collapses from 7 inline lines to a one-line call.

### 48.B — Streaming caller

In [`stream.rs`](../crates/tako-providers/vertex/src/stream.rs):

```rust
if let Some(fc) = part.function_call {
    tool_calls.push(ToolCallDelta {
        index: tool_call_index,
        id: Some(crate::convert::vertex_tool_call_id(&fc.name, &fc.args)),
        name: Some(fc.name),
        arguments_fragment: Some(fc.args.to_string()),
    });
    tool_call_index += 1;
}
```

`index` keeps doing what it already did — per-stream chunk
reassembly key. Operators reassembling fragments still
key on `index`; operators correlating cross-mode now key
on `id`.

### 48.C — Stream test

Add to [`tests/chat.rs`](../crates/tako-providers/vertex/tests/chat.rs):

```rust
#[tokio::test]
async fn stream_tool_call_id_matches_non_streaming() {
    // Build a Vertex SSE chunk that emits one tool call,
    // then a STOP terminator. Assert the streamed
    // ToolCallDelta::id equals the id `from_vertex_response`
    // would emit for the same (name, args) pair.
    ...
}

#[tokio::test]
async fn stream_distinct_tool_calls_get_distinct_ids() {
    // Two function_call deltas in one stream with
    // distinct (name, args). Assert their ids differ.
    ...
}
```

Both tests use the existing `wiremock` + `build_provider`
test fixture. No new infrastructure.

### 48.D — Version bump

0.48.0 → 0.49.0 across `Cargo.toml` (workspace + 14
internal crate version pins), `pyproject.toml`,
`python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 48.E — PLAN.md

- New row `48 — Stable Vertex tool-call IDs in streaming`.
- The "Phase 46 streaming follow-up" deferral is now
  closed (mention in PLAN.md backlog reasoning).

### 48.F — CHANGELOG `[0.49.0]`

Standard format, brief.

## Critical files

**Modified:**
- [`crates/tako-providers/vertex/src/convert.rs`](../crates/tako-providers/vertex/src/convert.rs) (48.A) — extract helper.
- [`crates/tako-providers/vertex/src/stream.rs`](../crates/tako-providers/vertex/src/stream.rs) (48.B) — use helper.
- [`crates/tako-providers/vertex/tests/chat.rs`](../crates/tako-providers/vertex/tests/chat.rs) (48.C) — 2 new tests.
- Standard PLAN/CHANGELOG/version flip.

**Created:**
- [`plans/PLAN_PHASE48.md`](PLAN_PHASE48.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
3. `cargo test -p tako-providers-vertex` — existing tests + 2 new stream tests pass.
4. `cargo test --workspace --exclude tako-py --all-features` — no regressions.
5. `ruff format --check` + `ruff check`.
6. `maturin develop --release` — wheel builds at v0.49.0.
7. `pytest -q` — full suite green; smoke pins v0.49.0.

## Out of scope

- **Other provider streaming paths.** OpenAI / Anthropic
  streaming chunks tool-call args incrementally (`fragment`
  is genuinely a fragment). The hash technique can't apply
  per-chunk — args aren't complete until end-of-call.
  Operators who need cross-mode correlation for those
  providers can compute the hash themselves on the
  reassembled call. Different problem; defer until ask.
- **Re-keying `ToolCallDelta::index`.** The `index` field
  remains the per-stream chunk-reassembly key. The hash
  goes into `id`, not `index`.
- **Deprecating `tool_call_index`.** Within-stream
  reassembly still needs an integer key. Even though
  Vertex emits one full call per delta and so chunks-per-
  call=1, removing `index` would break the streaming
  contract surface. Keep both fields, each with its
  documented purpose.
