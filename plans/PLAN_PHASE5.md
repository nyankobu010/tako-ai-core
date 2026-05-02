# PLAN — Phase 5 (production hardening)

> **Status: complete (v0.6.0, 2026-04-29).** All three explicit Phase 5
> candidates from [PLAN.md](PLAN.md) shipped. See
> [`CHANGELOG.md`](CHANGELOG.md) `## [0.6.0]` for the full diff.
>
> Successor to [PLAN_PHASE3.md](PLAN_PHASE3.md). Next milestone (Phase 6)
> is unscoped at the time of writing; see [PLAN.md](PLAN.md).

## Context

Phase 4 (v0.5.0) shipped on 2026-04-29: AB-MCTS orchestrator,
Mistral / Ollama providers, WebSocket + gRPC MCP transports, Sigstore
(keyed) tool-catalogue verification, Redis `BudgetBackend`, and
matching PyO3 + Python facades. Three follow-ups were explicitly
deferred in the source comments and changelog:

1. **Sigstore keyless verification** (Fulcio cert + identity policy) —
   `crates/tako-governance/src/sigstore.rs:13-18` deferred this off
   the keyed `CatalogueVerifier` landing.
2. **gRPC MCP mTLS / custom CAs** —
   `crates/tako-mcp/src/transport/grpc.rs:69-72` told users to wrap the
   tonic `Channel` manually if they needed it.
3. **Python orchestrator wiring for `BudgetBackend`** —
   `## [0.5.0]` Phase 4.G notes: the Redis backend was a standalone
   class; `tako.SingleAgent` / `tako.Client` did not consult it.

Phase 5 closes all three.

## What landed

### 5.A — Sigstore keyless verifier

`crates/tako-governance/src/sigstore.rs`. New `KeylessVerifier`
alongside the keyed `CatalogueVerifier`:

- Accepts a `KeylessBundle` (small JSON: `{leaf_cert_pem,
  signature_b64}`) — operator produces it from `cosign sign-blob`
  output in a few lines of shell.
- Enforces an `IdentityPolicy { issuer: String, san_match: SanMatch }`
  against the leaf cert's Fulcio v1 OIDC issuer extension
  (`1.3.6.1.4.1.57264.1.1`) and SAN. `SanMatch::Exact` or
  `SanMatch::Regex`.
- Validates the cert's `notBefore` / `notAfter` and Code Signing
  extended key usage.
- Verifies the signature using the cert's public key
  (`CosignVerificationKey::from_der` over the leaf's SPKI DER).

**v0.6.0 trust scope is leaf-cert + identity-policy + signature.**
Chain-of-trust validation against the Fulcio root and Rekor
inclusion-proof / SET verification are tracked as follow-ups; the
`verify_bundle` return shape will lift onto a chain-aware variant
without breaking callers. Operators are expected to validate those
pieces with `cosign verify-blob` at deploy time and ship a
pre-validated bundle. This sidesteps the heavy `sigstore` `verify`
feature (transitively requires `webbrowser` + `openidconnect`).

PyO3 binding `tako._native.KeylessVerifier` + Python facade
`tako.sigstore.KeylessVerifier(issuer, san, *, san_is_regex=False)`.
Tests (6 Rust + 4 Python) generate a Fulcio-style leaf cert at runtime
via `rcgen` (Rust) or `cryptography` (Python); no fixtures committed.
New example `examples/16_sigstore_keyless.py`.

### 5.B — gRPC MCP mTLS

`crates/tako-mcp/src/transport/grpc.rs`. New `connect_with_tls`
constructor takes `(endpoint, ca_pem, client_cert_pem,
client_key_pem, domain_name)`; the existing `connect` is unchanged.
Half-pair client identities (cert without key, etc.) are rejected
synchronously. The post-channel demux/spawn logic refactored into a
private `from_channel` helper so both constructors share it.

PyO3 binding `tako._native.Grpc` accepts the same kwargs (with
`bytes` types). Python facade `tako.mcp.Grpc(endpoint, *, ca_pem=,
ca_path=, client_cert_pem=, client_cert_path=, client_key_pem=,
client_key_path=, domain_name=)` reads PEM either inline or from a
filesystem path; the two are mutually exclusive.

Tests (4 Rust mTLS round-trips + 3 Python validation cases) generate
a self-signed CA + server cert + client cert at runtime via `rcgen`
and bind an in-process `tonic::transport::Server` with
`ServerTlsConfig::client_ca_root`. New example
`examples/17_grpc_mtls.py`.

`tako-mcp` gains a tiny dev-dep on `rustls` (with `aws_lc_rs`) so the
test binary can pin a CryptoProvider — both `aws-lc-rs` (via rcgen)
and `ring` (via tonic) end up linked, and rustls 0.23 refuses to
auto-pick when both are present.

### 5.C — `BudgetTracker` wired into the orchestrator API

*Rust:* `SingleAgent` and `SingleAgentBuilder` gain an optional
`Arc<BudgetTracker>` field plus `.budget(...)` builder method. In
both `Orchestrator::run` and `::stream`, every provider call is
preceded by `pre_check(principal, estimated_usd, est_tokens)` and
followed by `record(principal, estimated_usd, usage)`. The
pre-flight cost estimate uses `LlmProvider::estimate_cost_usd(&req)`
(the only cost hook on the trait); the post-call estimate reuses the
same value (per-token rates aren't yet provider-exposed). Pre-flight
token estimate is `req.max_tokens.unwrap_or(0)`. `BudgetExhausted`
errors short-circuit the run.

Conductor / Trinity / SelfCaller wiring is intentionally deferred —
SingleAgent is the v0.6.0 acceptance gate; the others reuse the same
pattern and will land in v0.7.0. No new public API surface is
disturbed.

*PyO3:* New `tako._native.InMemoryBudgetBackend` (always available)
alongside the redis-gated `RedisBudgetBackend`. Both expose
`current_usage(tenant_id)` / `record(tenant_id, usd, tokens)`
awaitables with the same shape. New `extract_budget_backend(py, obj)`
helper returns `Arc<dyn BudgetBackend>` from either pyclass.
`PyOrchestrator::new` gains `budget=` and `budget_backend=` kwargs;
when only `budget=` is set the backend defaults to in-memory.

*Python facade:*

- `tako.budget.InMemoryBackend` joins `tako.budget.RedisBackend` with
  the same async API.
- `tako.SingleAgent(provider, *, budget=, budget_backend=)` kwargs
  flow through.
- `tako.Client(budget=, budget_backend=)` stashes both so the
  README quickstart pattern works end-to-end.

Tests (2 Rust integration + 5 Python smoke) cover record
accumulation, pre-check short-circuit, kwarg acceptance, and Client
stashing. New example `examples/18_budget_wired.py`.

## Verification (Definition of Done — Phase 5)

```bash
# Rust
cargo fmt --all -- --check                          # clean
cargo clippy --workspace --all-targets -- -D warnings   # clean
cargo test --workspace                              # all green
cargo test -p tako-governance --features sigstore   # +6 keyless tests
cargo test -p tako-mcp --features grpc              # +4 mTLS tests
cargo test -p tako-orchestrator                     # +2 budget tests

# Python
maturin develop --release --features "sigstore grpc"
pytest -q tests/python                              # 85 passed (4 skipped)
ruff check python/ tests/python/ examples/          # clean
ruff format --check python/ tests/python/ examples/ # clean
mypy python/tako                                    # clean

# Wheel
python -c "import tako; print(tako.__version__)"   # → 0.6.0
```

## Acceptance gates

- [x] `KeylessVerifier::verify_bundle` round-trips a manifest signed
      via a Fulcio-style leaf cert + identity policy.
- [x] `GrpcTransport::connect_with_tls` succeeds against a server
      requiring a client cert; fails without one.
- [x] `tako.SingleAgent(provider, budget=Budget(max_tokens_per_request=N))`
      raises before the provider call when the cap is below the
      request's `max_tokens`.
- [x] `tako.SingleAgent(provider, budget=, budget_backend=InMemoryBackend())`
      records usage queryable via `await backend.current_usage(tenant)`.
- [x] All three new examples (`16_sigstore_keyless.py`,
      `17_grpc_mtls.py`, `18_budget_wired.py`) run without unhandled
      errors.
- [x] CHANGELOG `## [0.6.0]` entry added; version bumped to 0.6.0.

## Out of scope (intentional, with rationale)

- **Conductor / Trinity / SelfCaller budget wiring** — same pattern as
  SingleAgent; landing alongside other v0.7.0 hardening keeps the
  v0.6.0 cut focused.
- **Sigstore chain validation against Fulcio root** — operator
  pre-validates with cosign at deploy time. The `KeylessVerifier`
  return shape is forward-compatible with a chain-aware variant.
- **Sigstore Rekor SET verification** — same; the bundle JSON is
  forward-compatible.
- **AB-MCTS streaming** — genuinely complex tree-search interleaving;
  separate design effort.
- **OTLP in-process collector E2E test** — non-trivial fixtures for
  marginal coverage gain.

## Phase 6 (next milestone)

Unscoped at the time of writing. Likely candidates:

- Conductor / Trinity / SelfCaller `BudgetTracker` wiring (mirror 5.C).
- Sigstore chain-of-trust + Rekor SET (extend 5.A).
- Trinity streaming (carry-over from Phase 3 follow-ups).
