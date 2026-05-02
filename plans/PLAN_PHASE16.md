# PLAN — Phase 16 (Streaming-rollout backpressure + tako-compat auth hardening, continued)

## Context

Phase 15 (v0.16.0, 2026-05-01) closed the streaming-verifier
triumvirate: [`Verifier::evaluate_streaming`](/Users/kwc/tako-ai-core/crates/tako-core/src/traits/verifier.rs#L73-L79)
is now wired through Trinity (13.B), Conductor (14.A), and AB-MCTS
(15.A). It also extended the Phase-14 auth resolvers with Vault
dynamic token rotation (15.B.1) and OIDC RFC 7662 token introspection
(15.B.2).

Two production-grade gaps remained, both flagged in
[`PLAN.md`](PLAN.md) lines 56–65 as Phase-16 carry-forward, and both
closed in v0.17.0:

1. **Unbounded memory under slow consumers.** Both
   [`AbMcts::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L484-L485)
   and
   [`Conductor::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L543)
   funnelled per-delta `OrchEvent` / `WorkerStreamEvent` traffic
   through `tokio::sync::mpsc::unbounded_channel`. Consumers run the
   verifier inline (Conductor: lines 560-562; AbMcts: rollout
   future), so a slow `evaluate_streaming` impl — or any slow
   downstream sink — let producers pile up arbitrary memory before
   the consumer drained.
2. **Phase 15.B.2 deferred sub-items.** The OIDC introspection
   writeup (`oidc.rs:34-36`) explicitly deferred
   `introspection_endpoint_auth_method` selection — Phase 15 supports
   HTTP Basic only. RFC 7662 §2.1 defines `client_secret_post` as a
   sibling. The other long-standing Vault gap is **namespace
   support** (Vault Enterprise) — a single `X-Vault-Namespace` header
   on every KV lookup.

All four sub-items are strictly additive — public APIs unchanged
shape.

**Theme:** *Production hardening of the streaming and auth surfaces
shipped in 13–15.*

**Tag:** v0.17.0.

## A. Bounded mpsc backpressure for streaming verifier rollouts

### A.1 — AB-MCTS rollout channel

[`crates/tako-orchestrator/src/ab_mcts.rs`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L484-L496):
`unbounded_channel::<OrchEvent>()` → `channel::<OrchEvent>(ROLLOUT_EVENT_BUFFER)`
where `ROLLOUT_EVENT_BUFFER = 64`. Producer
([`rollout_static_streaming`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L948))
parameter type changes from `mpsc::UnboundedSender<OrchEvent>` to
`mpsc::Sender<OrchEvent>`; three `event_tx.send(...)` sites
(per-delta `AssistantText` line 1005, per-delta `VerifierScore`
line 1019, fallback-path `AssistantText` line 1084) gain the
trailing `.await`. The `let _ = ...` discard pattern stays —
consumer-drop on cancellation remains silent.

Consumer side ([`tokio::select!` recv-loop at lines 504-524](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L504-L524))
is unchanged — `rx.recv()` is identical on `Receiver`.

The `64` matches the
[`tako-mcp/src/transport/grpc.rs:45-46`](/Users/kwc/tako-ai-core/crates/tako-mcp/src/transport/grpc.rs#L45-L46)
precedent (`NOTIFICATION_BUFFER` / `OUTBOUND_BUFFER`).

### A.2 — Conductor worker fanout channel

[`crates/tako-orchestrator/src/conductor.rs:543`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L543):
same swap on `WorkerStreamEvent` —
`unbounded_channel` → `channel(WORKER_STREAM_BUFFER = 64)`.
[`dispatch_workers_streaming`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L831)
and
[`run_one_worker_streaming`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L891)
signatures change from `mpsc::UnboundedSender` to `mpsc::Sender`.
Two send sites
([`Done` line 869](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L869),
[`Delta` line 936](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L936))
gain the trailing `.await`.

### A.3 — Trinity is unchanged

[`Trinity::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/trinity.rs#L490-L596)
calls `evaluate_streaming` inline on the provider stream (no channel,
no fanout) — already serial. No work.

### A.4 — Slow-consumer regression tests

New `..._stream_bounded_backpressure_high_delta_count` tests in
[`ab_mcts_streaming_verifier.rs`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/tests/ab_mcts_streaming_verifier.rs)
and
[`conductor_streaming_verifier.rs`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/tests/conductor_streaming_verifier.rs):
drive 256 deltas (4× the 64-slot bound) through the channel under
the existing `CountingStreamingVerifier`. Pass iff every delta
crosses the channel without loss and the streaming-verifier hook
fires N times — i.e. backpressure neither drops events nor
deadlocks the producer.

## B. tako-compat auth hardening, continued

### B.1 — Vault Enterprise namespace support

[`crates/tako-compat/src/auth/vault.rs`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/vault.rs):
`VaultAuthResolver` gains an optional `namespace: Option<String>`
field and a chainable `with_namespace(namespace)` builder. The value
threads through
[`VaultClientSettingsBuilder::namespace`](https://docs.rs/vaultrs/0.7/vaultrs/client/struct.VaultClientSettingsBuilder.html)
in `get_or_build_client` so each cached `VaultClient` sends the
`X-Vault-Namespace` header on every KV lookup — required by Vault
Enterprise multi-tenant deployments. `None` (default) preserves
OSS-Vault behaviour byte-for-byte. Chainable on top of
`new` / `with_provider` / `with_approle` / `with_kubernetes` /
`with_kubernetes_in_pod` — namespace is orthogonal to auth method.

Four new unit tests cover the default-None case, single-builder set,
chain-with-other-builders, and presence-in-Debug.

### B.2 — OIDC introspection `client_secret_post` auth method

[`crates/tako-compat/src/auth/oidc.rs`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs):
new public `IntrospectionAuthMethod` enum
(`#[derive(Default)]`; default variant `ClientSecretBasic`) with a
`ClientSecretPost` alternative. `IntrospectionConfig` gains a public
`auth_method: IntrospectionAuthMethod` field — defaults to
`ClientSecretBasic`, so existing wire behaviour is byte-for-byte
preserved. New chainable
`OidcAuthResolver::with_introspection_auth_method(method)` setter
mutates the in-place introspection config; silent no-op when no
introspection has been attached yet.

`introspect()` branches on `auth_method`:
- **`ClientSecretBasic`** (existing): `Authorization: Basic`
  header, body carries `token` + `token_type_hint` only.
- **`ClientSecretPost`** (new): credentials in body
  (`client_id` / `client_secret` form fields), no Authorization
  header.

The `url::form_urlencoded::Serializer` is not `Send`, so the body
construction is wrapped in a tight scope that drops the serializer
before any await on the request.

Five new wiremock-based unit tests cover the default-Basic case, the
`with_introspection_auth_method` setter (override + no-op-without-config),
and conjugate wire-shape assertions for both auth methods.

Discovery-driven selection (RFC 8414
`introspection_endpoint_auth_methods_supported`),
`client_secret_jwt`, and mTLS auth methods remain deferred to Phase
17+.

### B.3 — Python facade mirror

[`crates/tako-py/src/py_compat.rs`](/Users/kwc/tako-ai-core/crates/tako-py/src/py_compat.rs):

- `tako.compat.VaultAuth.with_namespace(namespace)` — instance
  builder method (immutable; returns a fresh `VaultAuth`).
- `tako.compat.OidcAuth.with_introspection_auth_method(method)` —
  instance builder method. Accepts case-insensitive
  `"basic"` / `"client_secret_basic"` / `"post"` /
  `"client_secret_post"` aliases; raises `ValueError` on garbage
  input.

`#[derive(Clone)]` added to `VaultAuthResolver` so the facade can
implement the immutable-builder pattern (matches the existing
`OidcAuthResolver` cadence). `IntrospectionAuthMethod` re-exported
from `tako_compat` so the PyO3 binding can name the enum.

[`tests/python/test_phase16_auth.py`](/Users/kwc/tako-ai-core/tests/python/test_phase16_auth.py)
covers the facade attribute presence and chaining; Rust tests
remain the source of truth for behaviour.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  bounded-backpressure tests added in 16.A.1 / 16.A.2 pass; the new
  Vault-namespace tests in 16.B.1 pass; the new OIDC
  introspection-auth-method tests in 16.B.2 pass.
- `pytest -q tests/python/test_phase16_auth.py` — green on a wheel
  built with `--features "auth-jwt auth-oidc auth-vault"`.
- Existing Phase-13/14/15 streaming-verifier tests
  (`{ab_mcts,conductor,trinity}_streaming_verifier.rs`) still
  byte-for-byte green — no behavioural regression from the channel
  swap.

## Out of scope (Phase 17+)

- OIDC `client_secret_jwt` and mTLS (`tls_client_auth`)
  introspection auth methods.
- Discovery-driven `introspection_endpoint_auth_methods_supported`
  selection.
- OIDC refresh-token / end-session endpoint flows.
- Composite `AuthResolver`s (mTLS + bearer chaining).
- Vision / image content support across Anthropic / Vertex /
  Bedrock — warrants a dedicated phase, cross-cutting across three
  provider crates.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond) —
  needs a sandboxed runner.
- OTel end-to-end real-collector test.

## Commits

1. `feat(tako-orchestrator): bounded mpsc backpressure in AbMcts streaming rollout (Phase 16.A.1)`
2. `feat(tako-orchestrator): bounded mpsc backpressure in Conductor streaming dispatch (Phase 16.A.2)`
3. `feat(tako-compat): Vault Enterprise namespace support (Phase 16.B.1)`
4. `feat(tako-compat): OIDC introspection client_secret_post auth method (Phase 16.B.2)`
5. `feat(tako-py): Vault namespace + OIDC auth_method facade (Phase 16.B.3)`
6. `docs: Phase 16 PLAN/README/CHANGELOG flip (v0.17.0)`
