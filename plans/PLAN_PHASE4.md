# PLAN — Phase 4 (search & scale)

> **Status: complete (v0.5.0, 2026-04-29).** This plan doc is
> **retroactive** — it was reconstructed from
> [`CHANGELOG.md`](CHANGELOG.md) `## [0.5.0]` and the corresponding
> commits as part of the Phase 7 plan-doc restructure (2026-04-29). At
> the time Phase 4 landed there was no per-phase plan file; the
> "Phase 4" line in the legacy [PLAN.md](PLAN.md) "Done" list was the
> only summary.
>
> Successor: [PLAN_PHASE5.md](PLAN_PHASE5.md).

## Context

Phase 3 (v0.4.0) shipped `Trinity` (rule + ONNX router), `SelfCaller`
bounded recursion, the Python training + eval harnesses, and replaced
the Phase-2 streaming stubs in `SingleAgent` / `Conductor` with native
`OrchEvent` streams.

Phase 4's goal is **search + scale**: an AB-MCTS orchestrator with
verifiers (so test-time tree search joins the orchestrator family
alongside SingleAgent / Conductor / Trinity / SelfCaller), broader
provider coverage (Mistral + Ollama), broader MCP transport coverage
(WebSocket + gRPC), and the first production-grade trust + budget
plumbing (Sigstore tool-catalogue verification + Redis budget backend).

## What landed

### 4.A — AB-MCTS orchestrator

`crates/tako-orchestrator/src/ab_mcts.rs`. Adaptive Branching Monte
Carlo Tree Search with a `Verifier` trait for scoring leaf rollouts.
Uses Beta-distribution posteriors (`alpha`, `beta`) per node and
back-propagates verifier scores along the selection path. Public entry
point `AbMcts::run`; `stream` is stubbed (returns
`"not yet implemented"`) and tracked for a later phase.

### 4.B — Mistral + Ollama providers

`crates/tako-providers/mistral` and `.../ollama`. Mistral hits the
`/v1/chat/completions` endpoint (OpenAI-shaped); Ollama hits its
native `/api/chat` JSON. Both implement streaming + tool calls.

### 4.C — WebSocket MCP transport

`crates/tako-mcp/src/transport/websocket.rs`. Third `McpTransport`
impl alongside stdio + Streamable HTTP. Reader task demuxes inbound
JSON-RPC frames to per-id `oneshot`s and a broadcast channel for
notifications. Plaintext (`ws://`) and TLS (`wss://`).

### 4.D — gRPC MCP transport

`crates/tako-mcp/src/transport/grpc.rs`. Fourth `McpTransport`. Hand-
crafted `tako.mcp.bridge.v1.McpBridge.Open` bidi-streaming RPC carrying
opaque `Frame { bytes json }` messages. Plaintext (`http://`) +
webpki-roots TLS (`https://`); mTLS / custom CAs deferred to Phase 5.B.
Gated behind a new `grpc` Cargo feature on `tako-mcp` (`tonic = "0.14"`,
`prost = "0.14"`, `protoc-bin-vendored` so contributors don't need
system `protoc`).

### 4.E — Sigstore (keyed) tool-catalogue verification

`crates/tako-governance/src/sigstore.rs`. New `CatalogueVerifier`:
operator pins a public key (cosign default `--key`), the verifier
checks a base64 signature over a JSON `Catalogue { server, tools }`
manifest. Trust scope is **keyed**; keyless verification (Fulcio cert
+ Rekor) deferred to Phase 5.A. Gated behind a new `sigstore` Cargo
feature (`sigstore = "0.13", default-features = false, features =
["cert"]`).

### 4.F — Redis-backed `BudgetBackend`

`crates/tako-runtime/src/budget_redis.rs`. Multi-process backend
alongside Phase-1's in-memory one. Keys are
`<prefix>:{tenant_id}:{YYYY-MM-DD}` (UTC). `record()` is atomic via a
small Lua script (`HINCRBYFLOAT usd` + `HINCRBY tokens` + `EXPIRE` in
one round-trip). Plaintext (`redis://`) + TLS (`rediss://`). Gated
behind a new `redis` Cargo feature (`redis = "1.2"` with
`tokio-comp` + `tokio-rustls-comp` + `script` + connection-manager).

### 4.G — Python facade for Phase 4 additions

`tako-py` + `python/tako/`:

- `tako.mcp.{WebSocket, Grpc}` join `Stdio` / `Http`. Both run the
  `initialize` → `initialized` MCP handshake at construction.
- `tako.sigstore.CatalogueVerifier(pem)` (or `.from_pem_path`) +
  `tako.sigstore.Catalogue` (typed `tools: list[ToolSchema]`).
- `tako.budget.RedisBackend(url, key_prefix=, ttl_secs=)` with async
  `current_usage` / `record`.
- New `tako-py` Cargo features `ws`, `grpc`, `sigstore`, `redis`
  forward to the matching feature on the underlying crate. Wheel built
  with the desired subset, e.g.
  `maturin develop --features "ws grpc sigstore redis"`.

**Carry-over flagged at landing**: `RedisBudgetBackend` was exposed as
a standalone class but was **not** wired into `tako.SingleAgent` /
`tako.Client`. Closed in Phase 5.C — see
[PLAN_PHASE5.md](PLAN_PHASE5.md).

## Verification (snapshot 2026-04-29)

```bash
cargo fmt --all -- --check                                                  # clean
cargo clippy --workspace --all-targets --all-features -- -D warnings        # clean
cargo test --workspace                                                      # all green
cargo test -p tako-mcp --features grpc                                      # +4 grpc tests
cargo test -p tako-governance --features sigstore                           # +6 sigstore tests
cargo test -p tako-runtime --features redis  # auto-skip without REDIS_URL  # +6 redis tests

maturin develop --release --features "ws grpc sigstore redis"
pytest -q tests/python                                                      # all green
python -c "import tako; print(tako.__version__)"                            # → 0.5.0
```

## Out of scope (intentional, then resolved later)

- **Sigstore keyless verification** (Fulcio cert + identity policy) —
  resolved in Phase 5.A.
- **gRPC mTLS / custom CAs** — resolved in Phase 5.B.
- **`BudgetBackend` wired through `tako.SingleAgent` / `tako.Client`** —
  resolved in Phase 5.C.
- **`AbMcts::stream` native impl** — still deferred (tracked as Phase 8
  candidate per [PLAN.md](PLAN.md)).

## Phase boundaries

Phase 5 picks up the three explicit follow-ups above; AB-MCTS native
streaming is intentionally postponed because the design (interleaving
rollouts across branches and emitting verifier scores) is a separate
effort from the production-hardening work that defined Phase 5.
