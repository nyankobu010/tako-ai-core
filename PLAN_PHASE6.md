# PLAN — Phase 6 (production hardening, continued)

> **Status: complete (v0.7.0, 2026-04-29).** Both follow-ups flagged in
> [PLAN_PHASE5.md](PLAN_PHASE5.md) shipped. See
> [`CHANGELOG.md`](CHANGELOG.md) `## [0.7.0]` for the full diff.
>
> Successor to [PLAN_PHASE5.md](PLAN_PHASE5.md). Next milestone (Phase 7)
> is unscoped at the time of writing; see [PLAN.md](PLAN.md).

## Context

Phase 5 (v0.6.0) shipped on 2026-04-29: Sigstore `KeylessVerifier`
(leaf cert + identity policy), gRPC MCP mTLS, and `BudgetTracker`
wired through `SingleAgent` only. Two follow-ups were explicitly
deferred:

1. **Budget wiring carry-over** — `Conductor`, `Trinity`, and
   `LlmJudgeGuard` (the only non-orchestrator type with a direct
   provider call) bypassed budgets.
2. **Sigstore chain-of-trust + Rekor SET** — `KeylessVerifier`
   v0.6.0 only checked the leaf cert's identity binding and
   signature; chain walk to a pinned root and Rekor SET
   verification were deferred.

Phase 6 closes both. SelfCaller itself has no direct provider calls
(it delegates to its `inner` orchestrator), so it inherits the
inner's budget automatically — no separate wiring needed there.
Rekor inclusion-proof (Merkle) is deferred to Phase 7 to keep
v0.7.0's scope focused.

## What landed

### 6.A — Conductor budget wiring

`crates/tako-orchestrator/src/conductor.rs`. `Conductor` and its
builder gain an optional `Arc<BudgetTracker>` field plus a
`.budget(...)` builder method. Every coordinator and worker provider
call is wrapped in `pre_check` → `chat` → `record`. Workers run in
parallel via `tokio::spawn`; each spawned task clones the tracker and
runs its own pre/record. A `BudgetExhausted` from a worker collapses
into the worker's error outcome (a string in `WorkerResult::outcome`)
and is then surfaced via `fail_fast` if enabled.

3 new Rust tests in `crates/tako-orchestrator/tests/conductor.rs`
(record accumulation across coordinator + workers, pre-check
short-circuit on a daily-USD cap, build-time kwarg acceptance).

### 6.B — Trinity budget wiring

`crates/tako-orchestrator/src/trinity.rs`. Same shape: optional
`Arc<BudgetTracker>` field + `.budget(...)` builder. Pre-flight
check + post-call record wrap the chosen role's chat call in `run`,
and the `step_usage`-resolving branch in `stream` (covering both the
streaming success path and both non-streaming fallback paths with a
single pair of pre/record calls).

2 new Rust tests in `crates/tako-orchestrator/tests/trinity.rs`
(record after chat, pre-check short-circuit on a daily-USD cap).

### 6.C — LlmJudgeGuard budget wiring

`crates/tako-orchestrator/src/self_caller.rs`. `LlmJudgeGuard`
gains an optional `Arc<BudgetTracker>` field + `.with_budget(...)`
builder method. The `evaluate` impl wraps `judge.chat(...)` in
pre/record. `SelfCaller` itself does **not** grow a budget field;
its rustdoc notes that the inner orchestrator's budget covers
regular execution and the guard's `with_budget()` covers judge
calls.

1 new Rust test in `crates/tako-orchestrator/tests/self_caller.rs`
(`with_budget` records judge usage).

### 6.D — Sigstore chain-of-trust validation

`crates/tako-governance/src/sigstore.rs`. New `TrustRoot` struct
holding `Vec<x509_cert::Certificate>` for roots + intermediates,
loadable from concatenated PEM (`from_pem`) or filesystem paths
(`from_paths`). `KeylessVerifier::with_trust_root(TrustRoot)`
extends the v0.6.0 leaf-cert + identity-policy check.

`KeylessBundle` gains a backwards-compatible
`chain_pem: Option<String>` field (serde-default `None`).

`verify_bundle` now walks `leaf → intermediate(s) → root` when a
`TrustRoot` is set: each cert's signature is verified using its
issuer's SPKI; `notBefore` / `notAfter` are checked at every hop;
the chain must terminate at one of the pinned roots within 16
hops. Without a `TrustRoot`, behaviour is unchanged.

Implementation uses existing deps (`x509-cert`,
`sigstore::crypto::CosignVerificationKey`); the heavy `sigstore`
`verify` feature (transitively requires `webbrowser` +
`openidconnect`) stays out of the dep tree.

2 new Rust tests in
`crates/tako-governance/tests/sigstore.rs::chain` (chain validates
against a freshly-minted root via `rcgen`; tampered/wrong-root
bundle is rejected).

### 6.E — Rekor SET verification

Same file. New `RekorEntry { log_index, log_id, integrated_time,
canonicalized_body, set_b64 }` struct, plus a
`rekor: Option<RekorEntry>` field on `KeylessBundle` (serde-default
`None`). `KeylessVerifier::with_rekor_key(&[u8])` pins the Rekor
public-good ECDSA-P256 key.

When both a Rekor key is pinned and the bundle carries a Rekor
entry, `verify_bundle` reconstructs the canonical SET-signed JSON
(sorted keys, no whitespace, per Rekor v0.0.1) and verifies the
SET against the pinned key. Inclusion-proof (Merkle) verification
is intentionally deferred to Phase 7.

2 new Rust tests in
`crates/tako-governance/tests/sigstore.rs::rekor` (round-trip
against a runtime-minted Rekor key; tampered SET rejected).

### 6.F — PyO3 + Python facade

`crates/tako-py/src/`:

- `py_conductor.rs`, `py_trinity.rs`, `py_self_caller.rs`: each
  `#[new]` constructor gains `budget: Option<PyBudget>` and
  `budget_backend: Option<Py<PyAny>>` kwargs, threaded through
  `crate::py_runtime::extract_budget_backend`.
- `py_sigstore.rs`: new `PyTrustRoot` pyclass (`#[new]` accepts
  `roots_pem` + optional `intermediates_pem`; `#[staticmethod]
  from_paths`). `PyKeylessVerifier::__init__` gains `trust_root=`
  and `rekor_public_key_pem=` kwargs.
- `lib.rs`: registers `PyTrustRoot` behind the `sigstore` feature.

`python/tako/`:

- `orchestrator.py`: `Conductor` and `Trinity` accept
  `budget=None, budget_backend=None`.
- `guards.py`: `LlmJudge` accepts the same kwargs.
- `sigstore.py`: new `TrustRoot` class; `KeylessVerifier` accepts
  `trust_root=None, rekor_public_key_pem=None`.
- `_native.pyi`: stubs updated for all of the above.

3 new Python smoke tests
(`test_phase6_budget_{conductor,trinity,judge}.py`) plus
`test_phase6_sigstore_chain.py` (auto-skipped without the
`sigstore` feature).

### 6.H — Examples, docs, version

- New examples: `examples/19_budget_fanout.py`,
  `examples/20_sigstore_full_chain.py`. Both run end-to-end against
  runtime-minted fixtures.
- Workspace + path-dep versions bumped to `0.7.0`. `pyproject.toml`
  + `python/tako/__init__.py::__version__` updated.
- `CHANGELOG.md` `## [0.7.0]` entry added.
- `PLAN.md` updated: Phase 6 done; Phase 7 candidates listed.

## Verification

```bash
# Rust
cargo fmt --all -- --check                          # clean
cargo clippy --workspace --all-targets -- -D warnings   # clean
cargo test --workspace                              # all green
cargo test -p tako-orchestrator                     # +6 budget tests
cargo test -p tako-governance --features sigstore   # +4 sigstore tests

# Python
maturin develop --release --features "sigstore"
pytest -q tests/python                              # 86 passed (9 skipped)
ruff check python/ tests/python/ examples/          # clean
ruff format --check python/ tests/python/ examples/ # clean
mypy python/tako                                    # clean

python -c "import tako; print(tako.__version__)"   # → 0.7.0

# Examples (smoke-run)
python examples/19_budget_fanout.py
python examples/20_sigstore_full_chain.py
```

## Acceptance gates

- [x] `tako.Conductor(coordinator=, workers=, budget=, budget_backend=)`
      records usage from coordinator and every worker call.
- [x] `tako.Trinity(roles=, router=, budget=, budget_backend=)`
      records usage on the chosen role's call.
- [x] `tako.guards.LlmJudge(judge=, budget=, budget_backend=)`
      records the judge's own usage.
- [x] `tako.sigstore.KeylessVerifier(issuer, san, trust_root=)`
      round-trips a chain-validated bundle; tampering any cert fails.
- [x] `tako.sigstore.KeylessVerifier(issuer, san,
      rekor_public_key_pem=)` round-trips a SET-bearing bundle; a
      tampered SET fails.
- [x] CHANGELOG `## [0.7.0]` entry added; version bumped to 0.7.0.
- [x] `PLAN_PHASE6.md` written (this file) and `PLAN.md` updated.

## Out of scope (intentional, with rationale)

- **Rekor inclusion-proof (Merkle)** — separate from SET; deferred
  to Phase 7. The `RekorEntry` JSON shape is forward-compatible
  with an added `inclusion_proof` field.
- **`SelfCaller::stream` native impl** — Phase 4 stub; non-trivial
  due to mid-stream confidence evaluation. Phase 7 candidate.
- **AB-MCTS native streaming** — separate design effort.
- **cosign protobuf-bundle shim** — tracked since v0.6.0; future
  ergonomics pass.
- **OPA / Trinity router budget caps** — separate concern from
  token/USD budgets; not in v0.7.0.

## Phase 7 (next milestone, indicative)

- Rekor inclusion-proof verification (extend 6.E).
- `SelfCaller::stream` native impl.
- cosign protobuf-bundle → `KeylessBundle` adapter.
- AB-MCTS native streaming.
