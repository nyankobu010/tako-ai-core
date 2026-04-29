# PLAN — Phase 7 (production hardening, continued)

> **Status: in progress.** Successor to [PLAN_PHASE6.md](PLAN_PHASE6.md).
> Closes both follow-ups flagged in `## [0.7.0]`'s release notes plus
> the cosign protobuf-bundle ergonomics carry-over tracked since v0.6.0.

## Context

Phase 6 (v0.7.0) shipped 2026-04-29: `BudgetTracker` is now wired
through every orchestrator (`Conductor`, `Trinity`) plus the
`LlmJudgeGuard`, and `KeylessVerifier` does chain-of-trust validation
(`TrustRoot`) + Rekor SET verification. Three explicit follow-ups
were called out in the release notes:

1. **Rekor inclusion-proof (Merkle)** — the SET (Signed Entry
   Timestamp) is verified, but the inclusion proof against the Rekor
   tree head is not. The `RekorEntry` JSON shape was deliberately left
   forward-compatible with an added `inclusion_proof` field.
2. **`SelfCaller::stream` native implementation** — Phase 4 carry-over
   stub still returning `"SelfCaller streaming is Phase 4; use 'run'
   for now"`. Deferred through Phases 5 and 6.
3. **cosign protobuf-bundle → `KeylessBundle` adapter** — operator
   ergonomics. Tracked since v0.6.0; today operators must hand-convert
   `cosign sign-blob --bundle out.pb` output to JSON.

Phase 7 closes all three. **AB-MCTS native streaming** stays out of
scope (deferred to Phase 8) — the design (interleaving rollouts +
emitting verifier scores) is a separate effort from these three
contained closures.

## What this phase will land

### 7.0 — Plan-doc restructure

Per-phase plan docs become the source of truth for each phase; the
unversioned [PLAN.md](PLAN.md) becomes a high-level index + roadmap.

- New: [PLAN_PHASE1.md](PLAN_PHASE1.md) (extracted from PLAN.md inline
  body).
- New: [PLAN_PHASE4.md](PLAN_PHASE4.md) (retroactive — Phase 4 had no
  per-phase doc; reconstructed from CHANGELOG `## [0.5.0]`).
- New: this file (`PLAN_PHASE7.md`).
- Slimmed: [PLAN.md](PLAN.md) — phase index table + Phase 7+ roadmap;
  inline Phase 1 body removed.

### 7.A — Rekor inclusion-proof verification

`crates/tako-governance/src/sigstore.rs`.

Extends Phase 6.E. New `RekorInclusionProof { log_index, tree_size,
root_hash_hex, hashes_hex: Vec<String> }` plus an `inclusion_proof:
Option<RekorInclusionProof>` field on `RekorEntry` (serde-default
`None`, back-compat with v0.7.0 bundles).

New `verify_rekor_inclusion(entry, proof) -> Result<(), TakoError>`
sibling to the existing `verify_rekor_set` (sigstore.rs:769). Algorithm
is RFC 6962 §2.1 audit-path verification: leaf hash =
`SHA256(0x00 || canonicalized_body_bytes)`; iterate `hashes_hex` per
the bit-pattern of `(log_index, tree_size)`; assert final hash equals
`root_hash_hex`.

Wired into `verify_bundle`: when SET passes AND
`entry.inclusion_proof.is_some()` AND a Rekor key is pinned, also run
inclusion-proof verification.

Rekor checkpoint signature (`SignedNote` over the tree head) stays out
of scope — phase 8 candidate.

3 new Rust tests in `crates/tako-governance/tests/sigstore.rs::rekor`:
round-trip against a runtime-built tree of 5 leaves; tampered
audit-path hash rejected; mutated `root_hash_hex` rejected.

No Python facade change (the proof is pure data inside the bundle JSON;
serde handles the new field automatically).

### 7.B — Native `SelfCaller::stream`

`crates/tako-orchestrator/src/self_caller.rs`. Replaces the stub at
self_caller.rs:192-202.

Mirrors `Trinity::stream` (trinity.rs:367-668): clone owned state up
front, build an `async_stream::try_stream!` block. Per recursion
iteration: forward every event from `inner.stream(...)` verbatim;
intercept `OrchEvent::Final { output }` and capture it instead of
yielding. After the inner stream ends, call
`confidence.evaluate(&p, &out.text).await?`. If above threshold or at
max depth, yield the captured `Final` and return; otherwise rebuild
`current_input` (same logic as `run`) and loop.

Step numbers from inner streams pass through unchanged. Intermediate
`Final` events are intentionally swallowed — only the last iteration's
`Final` is forwarded, matching `run`'s semantics.

**`OrchEvent` enum is intentionally left unchanged** — adding a
`Recursion { depth, confidence }` variant would force a wire-format
bump on every consumer (Python, OpenAI-compat server). The implicit
signal "more `StepStart`s after a `Final`" indicates a guard rejection.
Deferred to Phase 8 if there's demand.

3 new Rust tests in `crates/tako-orchestrator/tests/self_caller.rs`:
single-pass when guard is confident, recursion-to-`max_depth` when
guard returns 0.0, inner `AssistantText` deltas arrive before any
`Final`.

PyO3: `PySelfCaller::stream` mirrors `PyTrinity::stream`. Python
facade: `SelfCaller.stream(prompt, ...) -> AsyncIterator[OrchEvent]`.
1 new Python smoke test (`tests/python/test_self_caller_stream.py`).

### 7.C — cosign protobuf-bundle adapter

`crates/tako-governance/src/sigstore.rs` + a new vendored
`cosign_bundle.rs` proto module gated behind a new
`sigstore-protobuf` Cargo feature (which itself depends on the
existing `sigstore` feature). The proto module is the minimal hand-
maintained subset of the Sigstore protobuf-specs `Bundle` v1 message
the adapter needs — vendored so default builds keep their dep tree
slim.

`KeylessBundle::from_cosign_protobuf(bytes: &[u8]) -> Result<Self,
TakoError>` does the structural translation: leaf cert →
`leaf_cert_pem`, remaining chain certs → `chain_pem`, message
signature → `signature_b64`, first `tlogEntries[]` →
`Some(RekorEntry { ..., inclusion_proof: ... })` (using 7.A's
inclusion-proof shape).

Naming is cosign-agnostic so other Sigstore-compatible signers can
reuse the path: the feature flag is `sigstore-protobuf`, the methods
are `from_protobuf_bundle` / `verify_protobuf_bundle`.

2 new Rust tests in
`crates/tako-governance/tests/sigstore.rs::protobuf`: round-trip
against a programmatically-built `Bundle` proto using the Phase 6
runtime cert fixtures; missing-signature variant rejected with a clear
error.

PyO3: `PyKeylessVerifier::verify_protobuf_bundle(manifest_bytes,
bundle_pb)`. Python facade: same name on `tako.sigstore.KeylessVerifier`.
1 new Python smoke test (auto-skip without the `sigstore-protobuf`
feature).

### 7.D — Examples, docs, version

- `examples/21_self_caller_stream.py` — runs `SelfCaller(...).stream()`
  against `FakeProvider`, prints deltas.
- `examples/22_sigstore_protobuf.py` — verifies a `cosign sign-blob
  --bundle out.pb` style payload (proto bytes).
- Workspace + path-dep versions bumped to `0.8.0`. `pyproject.toml` +
  `python/tako/__init__.py::__version__` updated.
- `CHANGELOG.md` `## [0.8.0]` entry added.
- `PLAN.md` phase index updated: Phase 7 → done.

## Verification (Definition of Done — Phase 7)

```bash
# Rust
cargo fmt --all -- --check                                                          # clean
cargo clippy --workspace --all-targets -- -D warnings                               # clean
cargo test --workspace                                                              # all green
cargo test -p tako-governance --features "sigstore sigstore-protobuf"               # +5 tests (3 incl, 2 protobuf)
cargo test -p tako-orchestrator                                                     # +3 self_caller stream tests

# Python
maturin develop --release --features "sigstore sigstore-protobuf"
pytest -q tests/python                                                              # +2 smoke tests, all green
ruff check python/ tests/python/ examples/                                          # clean
ruff format --check python/ tests/python/ examples/                                 # clean
mypy python/tako                                                                    # clean

python -c "import tako; print(tako.__version__)"                                    # → 0.8.0

# Examples (smoke-run)
python examples/21_self_caller_stream.py
python examples/22_sigstore_protobuf.py
```

## Acceptance gates

- [ ] `tako.sigstore.KeylessVerifier(issuer, san, rekor_public_key_pem=)`
      round-trips a bundle that carries an `inclusion_proof` field;
      tampered hash fails.
- [ ] `async for ev in tako.SelfCaller(...).stream(prompt): ...` yields
      inner `AssistantText` deltas live, then exactly one `Final` per
      recursion cycle, with the *last* `Final` being the outer caller's
      result.
- [ ] `tako.sigstore.KeylessVerifier(...).verify_protobuf_bundle(manifest,
      pb_bytes)` round-trips a programmatically-built cosign `Bundle`
      proto.
- [ ] CHANGELOG `## [0.8.0]` entry added; version bumped to 0.8.0.
- [ ] `PLAN_PHASE7.md` written (this file); `PLAN.md` updated to the new
      index format.

## Out of scope (intentional, with rationale)

- **AB-MCTS native streaming** — Phase 8. Most design-heavy of the v0.7.0
  candidate set; benefits from a dedicated planning pass.
- **Rekor checkpoint (`SignedNote`) verification** — orthogonal to the
  inclusion-proof itself (the checkpoint is a signature over the tree
  head, separate from the audit path against it). Phase 8 candidate.
- **`OrchEvent::Recursion` variant** — would expose recursion depth +
  confidence on the wire. Defer until a concrete consumer asks for it.
- **Streaming `ConfidenceGuard`** — confidence is still evaluated on the
  buffered final text. Streaming-aware guard design (e.g. early abort) is
  a separate effort.

## Phase 8 (next milestone, indicative)

- AB-MCTS native streaming.
- Rekor checkpoint (SignedNote / tree-head) verification.
- `OrchEvent::Recursion` variant once a consumer needs it.
- Streaming-aware `ConfidenceGuard`.
