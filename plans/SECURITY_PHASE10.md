# Security Review — Phase 10 (sigstore stack, v0.11.0)

> Standalone review artefact for the Phase 10 sigstore work. Not a
> threat model for the whole project; scope and assumptions are stated
> below. Companion to [SECURITY.md](SECURITY.md) (vulnerability-reporting
> policy) and [PLAN_PHASE10.md](PLAN_PHASE10.md) (the in-flight plan).

## Context

Phase 10.A introduces [`crates/tako-governance/src/sigstore_state.rs`](crates/tako-governance/src/sigstore_state.rs)
and the `tako._native.JsonStateStore` PyO3 wrapper — the **first
persistence write path** in the sigstore stack. Until v0.10.0, every
trust state lived in the `KeylessVerifier` instance and died with the
process; v0.11.0 turns the Phase 9.B Rekor checkpoint freshness anchor
into operator-owned on-disk state.

That makes this the right moment for an end-to-end review of the
keyless verifier path: the new state store, the verifier's freshness
anchor it interacts with, and the surrounding cert / Rekor verification
that has accreted across Phases 5 → 9.

Scope was settled with the user up front:

- **In:** sigstore stack end-to-end (Phase 10.A new code + the
  Phases 5–9 keyless path it integrates with).
- **Out:** Phase 10.B/C/D surfaces (no new trust boundary). Sigstore
  service compromise. Wheel supply chain (covered by `SECURITY.md`).

**No code changes land with this review.** Concrete fixes are filed as
a single "Sigstore security hardening" entry under
[PLAN.md → Phase 11 candidates](PLAN.md#phase-11-candidates-indicative-not-yet-committed).

## Trust model

| Boundary | Operator must guarantee | tako guarantees |
|---|---|---|
| OS filesystem under the state-file path | Only the tako process can write; the path is not a hostile symlink chain. | Atomic replace via `write-temp-then-rename`; first-boot semantics on missing file. |
| Wall clock used for `notBefore` / `notAfter` | A trusted clock (NTP-synced, no operator-controlled offset). | `check_validity_now` ([sigstore.rs:642–657](crates/tako-governance/src/sigstore.rs#L642-L657)) compares strictly, no skew tolerance. |
| OIDC issuer + SAN match | `IdentityPolicy` precisely names the expected signing identity. | Both the issuer extension (Fulcio v1 OID `1.3.6.1.4.1.57264.1.1` or v2 `…1.8`) and one of {rfc822, URI, DNS} SAN entries must match. |
| Rekor public key, when pinned | The pinned key is the real Rekor public-good ECDSA-P256 key. | SET signature, optional Merkle inclusion-proof, optional `SignedNote` checkpoint, and (Phase 9.B) monotonic-non-decreasing checkpoint `tree_size` across `verify_bundle` calls on the verifier instance. |
| Trust root, when pinned | Roots are real Fulcio (or private CA) roots; intermediates are correctly chained. | Up to 16-hop walk leaf → intermediates → root, signature-verifying every hop. |

What is **out of scope** for tako's verifier (and thus must be handled
upstream of it):

- Compromise of the Sigstore public-good services (Fulcio CA, Rekor
  log, public-good trust root).
- Vulnerabilities in the upstream `sigstore-rs`, `x509-cert`, or
  `aws-lc-rs` crates (covered by `cargo audit` in CI).
- The compiled-binary supply chain, including `tako`'s own Sigstore
  signing of release wheels (covered by [SECURITY.md → Supply chain](SECURITY.md)).

## Findings

Twelve findings, severity-ranked. Each entry: location, impact, and a
suggested fix shape (not a patch). The High and Medium items roll up
into the Phase 11 hardening entry; the Low items are documented for
posterity and to bound the review.

### High

#### H1 — TOCTOU between rollback check and `fetch_max` on the freshness anchor

**Location:** [crates/tako-governance/src/sigstore.rs:617–625](crates/tako-governance/src/sigstore.rs#L617-L625).

**What.** The Phase 9.B freshness-anchor advance is three separate
operations: an `Ordering::Relaxed` load of `rekor_min_tree_size`, a
comparison `checkpoint.tree_size < prev`, and a `fetch_max` of the
new value. Under concurrent `verify_bundle` calls on a shared
`Arc<KeylessVerifier>` (the documented use case — see the doc-comment
at [sigstore.rs:510–518](crates/tako-governance/src/sigstore.rs#L510-L518)
explicitly motivating "PyO3 wrapper" and "tower layer" sharing), the
load is not paired with the fetch_max.

**Impact.** Thread A reads `prev = 10` and yields. Thread B verifies a
bundle with `tree_size = 20` and advances the anchor to 20. Thread A
resumes, sees its stale `prev = 10`, and accepts a bundle with
`tree_size = 15` even though that is a strict regression from the
already-observed 20. Phase 9.B's docstring promises monotonic
non-decrease; the current implementation violates that under
contention.

**Fix shape.** Replace the load + compare + fetch_max with a
`compare_exchange_weak` loop that re-reads on every retry:

```rust
let mut prev = self.rekor_min_tree_size.load(Ordering::Acquire);
loop {
    if checkpoint.tree_size < prev {
        return Err(/* rollback */);
    }
    let next = checkpoint.tree_size.max(prev);
    match self.rekor_min_tree_size.compare_exchange_weak(
        prev, next, Ordering::AcqRel, Ordering::Acquire,
    ) {
        Ok(_) => break,
        Err(observed) => prev = observed,
    }
}
```

Or take a `Mutex<u64>` (the path is ms-scale verify work; the lock is
uncontested in single-threaded use and the contended cost is a few µs).

### H2 — State file has no integrity guard and inherits the process umask

**Location:** [crates/tako-governance/src/sigstore_state.rs:99–127](crates/tako-governance/src/sigstore_state.rs#L99-L127).

**What.** `JsonStateStore::save` writes plain JSON via `std::fs::write`,
which honours the process umask. On a host with `umask 022` (the
default on most Linux distributions and on macOS), the freshness-anchor
file lands `0644` — world-readable, group-writable on some installs.
There is no HMAC, signature, or post-write `chmod` to enforce the
implicit assumption that "only tako can mutate this file."

**Impact.** An attacker with write access to the state file (e.g. a
co-tenant on the same host, a leaked container volume, a misconfigured
backup-restore) can silently downgrade `rekor_min_tree_size` and re-
enable rollback acceptance for the next process boot. The verifier
itself fails closed against an unparseable file (`load` raises
`TakoError::Invalid`), but a syntactically-valid downgrade is
indistinguishable from legitimate first-boot state.

**Fix shape.**

1. After the `rename` succeeds, call `std::fs::set_permissions(&path,
   PermissionsExt::from_mode(0o600))` on Unix targets.
2. Surface the file-permission requirement in the rustdoc on
   [sigstore_state.rs:60–98](crates/tako-governance/src/sigstore_state.rs#L60-L98)
   and in the Python facade docstring at
   [python/tako/sigstore.py:230–257](python/tako/sigstore.py#L230-L257).
3. Add a one-line note to `examples/23_state_store.py` showing the
   recommended `umask 077` posture for the state directory.

A detached-HMAC sidecar file is a heavier option that's only worth it
if H2 + a second compromise vector ever line up; document but defer.

### Medium

#### M1 — Concurrent `save()` on the same instance races on a deterministic tmp path

**Location:** [crates/tako-governance/src/sigstore_state.rs:116–127](crates/tako-governance/src/sigstore_state.rs#L116-L127),
[:146–153](crates/tako-governance/src/sigstore_state.rs#L146-L153).

**What.** `tmp_path()` returns `<file>.tmp` deterministically. Two
concurrent `save()` calls on a shared `Arc<JsonStateStore>` (plausible
via the PyO3 wrapper, which exposes `&self` methods) interleave their
writes and renames; the loser's `rename` either silently overwrites
the winner's value (data loss) or fails leaving an orphan `.tmp`.

**Impact.** Worst case is a corrupted persisted value that the next
`load()` rejects with `TakoError::Invalid("parse …")` — fail-closed,
but loud and hard to root-cause, and the rejected file still sits on
disk until the operator intervenes.

**Fix shape.** Use `tempfile::NamedTempFile::new_in(parent).persist()`
which generates a randomised suffix and handles the rename atomically.
Alternative: append `pid + counter + nanos` to the tmp suffix.

#### M2 — Missing `#[serde(deny_unknown_fields)]` on `StateFile`

**Location:** [crates/tako-governance/src/sigstore_state.rs:55–58](crates/tako-governance/src/sigstore_state.rs#L55-L58).

**What.** `StateFile` accepts any extra JSON fields silently. There is
also no `version` field.

**Impact.** Forward-incompatible: a future schema with (say) a SET
timestamp anchor or a per-checkpoint signature would land alongside
the existing `rekor_min_tree_size` field, and an old reader on a new
file would silently drop the new field — masking a misconfiguration
where the operator believes the new anchor is in force. This is the
exact failure mode a transparency log is meant to make visible.

**Fix shape.** Add `#[serde(deny_unknown_fields)]` and a `version: u32`
field defaulted to `1`. Bump on schema change; reject anything else.

#### M3 — `verify_chain` does not check `basicConstraints: cA=TRUE` on intermediates / roots

**Location:** [crates/tako-governance/src/sigstore.rs:829–887](crates/tako-governance/src/sigstore.rs#L829-L887).

**What.** The chain-walk picks issuers by exact `subject == issuer`
DN match (sigstore.rs:854–873) and signature-verifies the link, but
never inspects the `BasicConstraints` extension to confirm the issuer
cert is itself a CA. RFC 5280 §4.2.1.9 makes `cA=TRUE` mandatory on
any cert that signs other certs.

**Impact.** Mostly defence-in-depth: the operator-supplied trust root
is already curated, and forging a chain through a non-CA cert requires
an attacker to control a private key whose public key sits in the
trust store. But if a misconfigured trust root ever included a
non-CA leaf as an intermediate, tako would happily walk through it.
Real-world Fulcio + sigstore-rs already enforce this; tako's own walk
should too. `pathLenConstraint` (RFC 5280 §4.2.1.9) is in the same
boat — Fulcio sets it, tako ignores it.

**Fix shape.** In [`verify_chain`](crates/tako-governance/src/sigstore.rs#L829),
after picking an issuer at each hop, parse `BasicConstraints`, require
`cA == true`, and (if `pathLenConstraint` is present) enforce it
against the depth remaining. Reject unknown critical extensions while
we're in there (RFC 5280 §4.2 mandates).

#### M4 — No tmp-file cleanup on `rename` failure

**Location:** [crates/tako-governance/src/sigstore_state.rs:120–127](crates/tako-governance/src/sigstore_state.rs#L120-L127).

**What.** If `rename` returns `Err`, the `.tmp` file stays behind. A
subsequent successful `save()` overwrites the tmp via `fs::write`, but
between the failed save and the next attempt, the tmp file is visible
and confusing.

**Impact.** Operator-experience and forensics, not a security primitive.

**Fix shape.** Wrap the tmp file in a guard struct whose `Drop` impl
deletes the file unless `persist()` was called explicitly — the
`tempfile::NamedTempFile` API already does this.

### Low / defence-in-depth

#### L1 — `create_dir_all` follows symlinks

**Location:** [crates/tako-governance/src/sigstore_state.rs:105–113](crates/tako-governance/src/sigstore_state.rs#L105-L113).

If a component of the parent path is a symlink, `create_dir_all`
follows it. This is OS-default behaviour and the operator chose the
path; downgrade-by-symlink-redirect is a strictly weaker attack than
H2 (an attacker who can plant symlinks in the tako state directory
can already meet H2's preconditions). Note for completeness; no fix
recommended.

#### L2 — `extract_san_value` returns the first acceptable SAN, ignoring later ones

**Location:** [crates/tako-governance/src/sigstore.rs:680–701](crates/tako-governance/src/sigstore.rs#L680-L701).

The loop returns the first `Rfc822Name` / `UniformResourceIdentifier`
/ `DnsName`. Fulcio currently emits a single SAN per cert, so this is
defence-in-depth — but RFC 5280 permits multiple SANs, and a future CA
or a compromised issuance pipeline could include both a benign-looking
SAN and an attacker-chosen one. Either iterate all SANs and require
**any** match, or require **exactly one** SAN.

#### L3 — Hand-rolled SET canonicalisation in `verify_rekor_set`

**Location:** [crates/tako-governance/src/sigstore.rs:928–942](crates/tako-governance/src/sigstore.rs#L928-L942).

The canonical JSON is built via `format!` with no input escaping. In
practice the inputs are constrained: `body` is base64, `log_id` is hex,
`integrated_time` is `i64`, `log_index` is `u64`. None can hold quotes
or backslashes, so escaping isn't load-bearing today. But the code
silently relies on those input invariants holding for all eternity.
Replace with `serde_json::to_string` over a `BTreeMap` (sorted-key
canonical form), or a tiny dedicated canonical-JSON helper.

#### L4 — No multi-threaded regression test for the freshness anchor

**Location:** [crates/tako-governance/tests/sigstore.rs:1375–1565](crates/tako-governance/tests/sigstore.rs#L1375-L1565).

The Phase 9.B integration tests
(`monotonic_ascent_accepted_and_advances_high_water_mark`,
`rollback_rejected_after_higher_tree_size_observed`,
`seed_then_verify_then_persist_round_trips_high_water_mark`) are all
single-threaded. H1 has no regression test today; the fix needs one.

#### L5 — OIDC issuer v1 extraction reads raw `extn_value` bytes without ASN.1 framing

**Location:** [crates/tako-governance/src/sigstore.rs:709–717](crates/tako-governance/src/sigstore.rs#L709-L717).

Matches Fulcio's actual encoding (the comment says so) and the
Sigstore community has not changed it. But if a CA ever emitted the v1
extension with proper IA5String DER framing, tako would compare DER
bytes against the URI string and fail to match — a hard-to-diagnose
interop break. Optional: try unframed first, fall back to IA5String
decode.

## Test gaps to file alongside the fixes

- **Multi-threaded `verify_bundle` stress** for H1: spawn N tokio
  tasks against a shared `Arc<KeylessVerifier>` with bundles spanning
  `tree_size` ∈ [1, 10_000], assert no rollback was ever accepted post-
  hoc by replaying observed checkpoints in chronological order.
- **Tampered state-file** integration test for H2: write a downgrade
  value to the state file out-of-band, reload, verify a fresh checkpoint
  whose `tree_size` is between the tampered value and the previous
  high-water mark, assert rejection.
- **`chmod 0600` smoke test** for H2 on Unix: after `save()`, assert
  `metadata().permissions().mode() & 0o777 == 0o600`.
- **`basicConstraints` enforcement** for M3: regression cert chain with
  a non-CA "intermediate"; assert chain rejection.
- **`deny_unknown_fields` regression** for M2: a JSON file with an
  extra `attacker_field` must round-trip-fail rather than being silently
  ignored.

## Recommended Phase 11 work (advisory)

A single line is appended to [PLAN.md → Phase 11 candidates](PLAN.md)
and to [PLAN_PHASE10.md → Phase 11 candidates (carry-forward)](PLAN_PHASE10.md):

> **Sigstore security hardening (review-driven).** Land H1 + H2 + M1–M4
> from `SECURITY_PHASE10.md`: race-free freshness-anchor advance
> (`compare_exchange_weak` or `Mutex<u64>`), `0600` state-file mode +
> docstring, unique tmp filenames, `deny_unknown_fields` + schema
> `version`, `basicConstraints: cA=TRUE` enforcement in `verify_chain`,
> tmp cleanup on rename failure. Strictly additive; no public API
> change.

L1–L5 land opportunistically alongside the H/M work; none warrant
their own phase.

## Findings considered and dismissed

For transparency. The exploratory pass surfaced these; the calibrated
review explicitly does **not** treat them as findings:

- **"Python `JsonStateStore.seed()` mutates the verifier in place via
  reference reassignment."** The wrapper at
  [python/tako/sigstore.py:279–285](python/tako/sigstore.py#L279-L285)
  reassigns `verifier._native`, but the underlying Rust call is
  `KeylessVerifier::set_rekor_min_tree_size`, which uses an
  `Arc<AtomicU64>` and is documented `&self` precisely because it's
  shareable ([sigstore.rs:510–518](crates/tako-governance/src/sigstore.rs#L510-L518)).
  The reassignment is cosmetic; both the old and new `_native` point
  to the same Rust object. Not a finding.
- **"GIL discipline broken in the seed path."** The PyO3 binding at
  [crates/tako-py/src/py_sigstore.rs:298–309](crates/tako-py/src/py_sigstore.rs#L298-L309)
  takes the borrow correctly, and `inner.load()` is pure Rust file
  I/O with no Python re-entry. Not a finding.
- **"`u64::MAX` injection into the state file is privilege escalation."**
  An attacker who can write the file can already cause arbitrary
  rollback acceptance via H2; setting `u64::MAX` instead causes
  unconditional rejection of every subsequent checkpoint, which is a
  DoS not an EoP. Reframed under H2 ("file tampering").

## Out of scope for this review

- Phase 10.B (named SSE events): emits operator-controlled scores to
  operator-controlled clients on the same host as the tako process.
  No new trust boundary; reviewed only for "could the JSON encoding
  of `tako.tool_call_result.result` carry a payload that mis-frames
  the SSE stream", and the answer is no — `serde_json::to_string`
  escapes newlines.
- Phase 10.C (`Verifier` in Trinity / Conductor): the `Verifier`
  trait runs in-process, identically to the Phase 8 `AbMcts` wiring
  that has been in production since v0.9.0. No new trust boundary.
- Phase 10.D (Python custom provider streaming): the streaming impl
  inherits the Phase 1 `chat()` GIL discipline pattern documented in
  [ARCHITECTURE.md → Async + GIL discipline](ARCHITECTURE.md). The
  test at `tests/python/test_phase10_python_streaming.py` (when
  written) needs to assert that dropping the Rust stream cancels the
  Python coroutine, but that's correctness, not a trust boundary.
- Sigstore service compromise (Fulcio, Rekor public-good).
- Wheel supply chain (covered by `SECURITY.md → Supply chain`).
- Upstream `sigstore-rs` / `x509-cert` / `aws-lc-rs` vulnerabilities
  (covered by `cargo audit` in CI per `SECURITY.md`).
