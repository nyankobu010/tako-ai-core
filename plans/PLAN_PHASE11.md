# PLAN — Phase 11 (Sigstore security hardening + http-generic provider streaming)

## Context

Phase 10 (v0.11.0, 2026-04-30) shipped:

- On-disk `JsonStateStore` for the Phase 9.B Rekor checkpoint freshness anchor (10.A).
- `tako.tool_call_start` / `tako.tool_call_result` named SSE events in `tako-compat` (10.B).
- Optional `Verifier` in `Trinity` and `Conductor`, mirroring the Phase 8 `AbMcts` wiring (10.C).
- Real Python custom-provider streaming via an optional `stream=` async-generator kwarg (10.D).
- Examples 23–26, README sweep, CHANGELOG `## [0.11.0]` (10.E).

Two follow-ons remain pre-loaded in the roadmap:

1. The Phase 10.A persistence write-path triggered an end-to-end review of the keyless verifier — see [SECURITY_PHASE10.md](/Users/kwc/tako-ai-core/SECURITY_PHASE10.md). The review surfaced two High and four Medium findings with concrete fix shapes; all are strictly additive. The single-line "Sigstore security hardening (review-driven)" entry already lives at [PLAN.md → Phase 11 candidates](/Users/kwc/tako-ai-core/PLAN.md) and the carry-forward block of [PLAN_PHASE10.md](/Users/kwc/tako-ai-core/PLAN_PHASE10.md).
2. The Phase 2 streaming stale marker at [crates/tako-providers/http-generic/src/lib.rs:259](/Users/kwc/tako-ai-core/crates/tako-providers/http-generic/src/lib.rs#L259) is the last unstreamed first-party provider. Phase 10's out-of-scope note already designed the shape: a `StreamConfig` enum with OpenAI-compat SSE and NDJSON variants, and a JSON-pointer-style delta extractor.

Phase 11 lands both: the security review write-up and the http-generic streaming gap. Larger discrete items (MCP Streamable HTTP SSE, vision content, eval-harness graders, Redis-backed StateStore, streaming-aware verifier in Trinity/Conductor) defer to Phase 12+ for the same reasoning given in Phase 10.

**Theme:** *Sigstore security hardening + close one Phase 2 streaming stale marker.*

**Target tag:** v0.12.0.

## What this phase will land

### 11.0 — Plan-doc + version

- New per-phase plan doc: this file, mirror of [PLAN_PHASE10.md](/Users/kwc/tako-ai-core/PLAN_PHASE10.md).
- [PLAN.md](/Users/kwc/tako-ai-core/PLAN.md) phase-index table: add Phase 11 row, status `in progress`, then flip to `done (date)` at end of phase.
- Workspace package version: `0.11.0` → `0.12.0` in `Cargo.toml` (workspace + every per-crate `version =`), `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 11.A — Sigstore security hardening (review-driven)

Lands H1 + H2 + M1–M4 from [SECURITY_PHASE10.md](/Users/kwc/tako-ai-core/SECURITY_PHASE10.md), plus opportunistic L2–L4 (and L5 documentation). Strictly additive — no public Rust or Python signature changes; behaviour is byte-for-byte identical when the freshness anchor sees no concurrent access and the operator never tampers with the state file.

#### H1 — Race-free freshness-anchor advance

**File / line:** [crates/tako-governance/src/sigstore.rs:617–625](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L617-L625).

**Change shape.** Replace the `Ordering::Relaxed` load + compare + `fetch_max` triple with a `compare_exchange_weak` loop on `Acquire` / `AcqRel` / `Acquire` orderings. The loop re-reads `prev` on every retry so a concurrent advance cannot create a stale-comparison window. The rollback check stays inside the loop so a checkpoint that regresses against any observed value (not just the value at loop entry) is rejected.

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

The `set_rekor_min_tree_size` setter at [sigstore.rs:516–517](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L516-L517) and the getter at `:524` are both upgraded to `Release` / `Acquire` ordering for symmetry; this is invisible to callers but makes the cross-thread happens-before edge explicit.

#### H2 — `0o600` mode on the state file (Unix)

**File / line:** [crates/tako-governance/src/sigstore_state.rs:99–127](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state.rs#L99-L127).

**Change shape.** After the successful `rename`, on `cfg(unix)` call `std::fs::set_permissions(&self.path, std::os::unix::fs::PermissionsExt::from_mode(0o600))`. Failure to chmod returns `TakoError::Invalid` so a misconfigured filesystem (mode set fails on tmpfs without ACL) is loud. On Windows, the chmod is a no-op with a one-line `// state-file confidentiality is operator-managed via NTFS ACL on Windows` comment.

Documentation:

- New rustdoc paragraph above `JsonStateStore` ([sigstore_state.rs:48–58](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state.rs#L48-L58)) noting "the state file is chmod'd to 0o600 on Unix; on Windows the operator must set NTFS ACLs." Cross-link to [SECURITY_PHASE10.md → H2](/Users/kwc/tako-ai-core/SECURITY_PHASE10.md#h2--state-file-has-no-integrity-guard-and-inherits-the-process-umask).
- Mirror the paragraph into the Python facade docstring at [python/tako/sigstore.py:230–257](/Users/kwc/tako-ai-core/python/tako/sigstore.py#L230-L257).
- Add a one-line comment in [examples/23_state_store.py](/Users/kwc/tako-ai-core/examples/23_state_store.py) recommending `umask 077` for the parent directory when tako runs as a service user.

#### M1 — Unique tmp filenames via `tempfile::NamedTempFile`

**File / line:** [crates/tako-governance/src/sigstore_state.rs:146–153](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state.rs#L146-L153) (deterministic `<file>.tmp`).

**Change shape.** Replace `tmp_path()` + `fs::write` + `fs::rename` with a `tempfile::NamedTempFile::new_in(parent)` followed by `tmp.write_all(&body)?; tmp.persist(&self.path)?`. `NamedTempFile` produces a randomised suffix (so two concurrent saves on the same `Arc<JsonStateStore>` cannot collide) and its `Drop` impl deletes the file if `persist` is never called — which subsumes M4 entirely.

**Cargo plumbing.** `tempfile = "3"` is currently dev-dep only at [crates/tako-governance/Cargo.toml:65](/Users/kwc/tako-ai-core/crates/tako-governance/Cargo.toml#L65). Promote to `[dependencies]`. The `[dev-dependencies]` line stays — Cargo deduplicates.

#### M2 — `#[serde(deny_unknown_fields)]` + schema `version`

**File / line:** [crates/tako-governance/src/sigstore_state.rs:55–58](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state.rs#L55-L58).

**Change shape.**

```rust
const STATE_FILE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    #[serde(default = "default_version")]
    version: u32,
    rekor_min_tree_size: u64,
}

fn default_version() -> u32 { STATE_FILE_VERSION }
```

`load()` rejects any `version != STATE_FILE_VERSION` with `TakoError::Invalid("sigstore_state: unsupported state file version {n}; rebuild from a fresh boot")`. `save()` always writes `version: 1`. The `default = …` keeps existing v0.11.0 state files (no `version` field) loadable as v1 — that's the on-disk migration story.

#### M3 — `BasicConstraints: cA=TRUE` + `pathLenConstraint` enforcement in `verify_chain`

**File / line:** [crates/tako-governance/src/sigstore.rs:829–887](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L829-L887), with the issuer-pick at [:854–873](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L854-L873).

**Change shape.** After the issuer cert is selected at each hop, parse the `BasicConstraints` extension (OID `2.5.29.19`, mandatory per RFC 5280 §4.2.1.9 for any cert that signs other certs):

- Reject the chain if the issuer has no `BasicConstraints` or `cA == false`.
- If `pathLenConstraint` is `Some(n)`, reject the chain if there are more than `n` non-self-issued intermediate hops *remaining below this issuer in the chain*.
- Iterate the issuer's extension list; reject any `critical: true` extension whose OID is not in the known-handled set (`BasicConstraints`, `KeyUsage`, `ExtendedKeyUsage`, `SubjectAltName`, `SubjectKeyIdentifier`, `AuthorityKeyIdentifier`, plus the two Fulcio OIDC OIDs already parsed). RFC 5280 §4.2 mandates the reject.

The leaf cert is *not* required to be a CA — only the issuers it walks through are. Existing leaf-cert tests stay green.

#### M4 — Tmp cleanup on rename failure

Subsumed by M1. `NamedTempFile::persist` returns the original tmp on `Err`, and the `Drop` impl removes it. No separate code change needed; call this out in the M4 commit message.

#### L2 — `extract_san_value` requires *any* SAN match across the full list

**File / line:** [crates/tako-governance/src/sigstore.rs:680–701](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L680-L701).

**Change shape.** Replace the "first match wins" early-return with a loop over the entire `GeneralNames` set and a final boolean return: any SAN that satisfies the requested predicate wins; if multiple SANs match, that's fine. Reject only when zero SANs match. Net behavioural change: a Fulcio cert with a single benign SAN + an attacker-injected SAN that happens to be earlier in the list cannot win — the predicate sees both.

#### L3 — `serde_json::to_string` over `BTreeMap` for canonical SET payload

**File / line:** [crates/tako-governance/src/sigstore.rs:928–942](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L928-L942).

**Change shape.** Replace the hand-rolled `format!`-based JSON canonicalisation with a `BTreeMap<&'static str, serde_json::Value>` insert + `serde_json::to_string`. `BTreeMap` gives the sorted-key canonical form Rekor SET wants, and `serde_json` handles all escape rules. Existing fixtures at `crates/tako-governance/tests/sigstore.rs` continue to verify-cleanly because the chosen inputs are RFC 7159-equivalent.

#### L4 — Multi-threaded freshness-anchor regression test

**File:** new test in [crates/tako-governance/tests/sigstore.rs](/Users/kwc/tako-ai-core/crates/tako-governance/tests/sigstore.rs) (after the existing single-threaded suite at lines 1375+) — *not* a separate file, to keep the freshness-anchor tests co-located with their setup helpers. Test `multi_threaded_advance_never_observes_rollback`: spawn 16 `tokio::spawn` tasks against a shared `Arc<KeylessVerifier>`, each calling `verify_bundle` against a randomly-ordered sequence of `tree_size` ∈ [1, 1_000]; collect the observed `rekor_max_tree_size()` after every successful verify; assert the sequence is monotonic-non-decreasing across the union.

#### L5 — Documentation only

**File / line:** [crates/tako-governance/src/sigstore.rs:709–717](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L709-L717).

Add a doc comment to `extract_oidc_issuer_v1` noting the unframed-bytes assumption and pointing at the v2 OID handler for the IA5String-framed alternative. No code change. If a future Fulcio version flips encodings the comment becomes the breadcrumb.

#### Test list — 11.A

All tests live under `crates/tako-governance/tests/`. No new test crate; `sigstore_state.rs` and `sigstore.rs` test files already exist.

**`tests/sigstore_state.rs` (new tests file at the integration-test layer; existing in-module unit tests stay):**

- `tampered_state_file_with_extra_field_is_rejected` — write `{"version":1,"rekor_min_tree_size":42,"attacker_field":"x"}`; assert `load()` returns `TakoError::Invalid("…unknown field `attacker_field`…")`.
- `unknown_version_is_rejected` — write `{"version":2,"rekor_min_tree_size":42}`; assert `load()` returns `TakoError::Invalid("…unsupported state file version 2…")`.
- `legacy_no_version_field_loads_as_v1` — write `{"rekor_min_tree_size":17}` (the v0.11.0 shape); assert `load()` returns `Ok(17)`.
- `unix_save_chmods_state_file_to_0o600` — `cfg(unix)` only; after `save(99)`, assert `metadata().permissions().mode() & 0o777 == 0o600`.
- `concurrent_save_does_not_collide` — spawn 8 threads each calling `save(thread_id as u64)` against a shared `Arc<JsonStateStore>`; assert all calls return `Ok` and the final `load()` returns one of the eight written values (no `EBUSY`, no orphan tmp).
- `tmp_residue_absent_after_rename_failure` — point the store at a path where the parent directory is read-only mid-test; assert the failed `save` leaves no `*.tmp` files.

**`tests/sigstore.rs` additions (new sub-mod `mod hardening { … }`):**

- `multi_threaded_advance_never_observes_rollback` — described in L4 above.
- `non_ca_intermediate_chain_is_rejected` — build (via `rcgen`) a chain leaf → "intermediate" (no `BasicConstraints` extension) → root; verify `BundleSpec` against this chain; assert `Err(TakoError::Invalid("…basicConstraints…"))`.
- `path_len_constraint_enforced` — build chain with `pathLenConstraint: 0` on the intermediate, then a second intermediate below it, then a leaf; assert the chain is rejected when 2 intermediate hops follow the constrained issuer.
- `unknown_critical_extension_is_rejected` — build chain with an unknown OID marked critical on the intermediate; assert chain rejection.
- `multi_san_predicate_match_succeeds` — build a leaf cert with SANs `["adversary@evil.example", "trusted@example.com"]`; assert the chain is accepted when the `IdentityPolicy` matches `trusted@example.com` (per the L2 fix the predicate sees all SANs).
- `multi_san_no_match_rejected` — same setup, identity policy that matches neither SAN; assert chain rejected.
- `canonical_set_round_trips_through_btreemap` — fixture-driven; assert the new canonical SET form is byte-equal to the v0.11.0 form for every existing fixture.

**`tests/python/test_phase11_state_store_hardening.py`** — Python smoke that `os.stat`s the state file after a `JsonStateStore.save`, asserts `0o600` mode on `cfg(unix)`-equivalent (`platform.system() in {"Linux", "Darwin"}`).

**Public API risk:** zero. No public Rust function changes signature; no public Python attribute changes. The Cargo dep promotion of `tempfile` is a no-op for downstream wheels (already pulled in transitively).

### 11.B — `http-generic` provider streaming

Closes the Phase 2 stale marker at [crates/tako-providers/http-generic/src/lib.rs:253–261](/Users/kwc/tako-ai-core/crates/tako-providers/http-generic/src/lib.rs#L253-L261). Mirrors the OpenAI SSE parser at [crates/tako-providers/openai/src/stream.rs](/Users/kwc/tako-ai-core/crates/tako-providers/openai/src/stream.rs) — reuses `eventsource-stream` (already a workspace dep), reuses the `[DONE]` sentinel handling, reuses the `ChatChunk::Delta` / `ChatChunk::End` shape.

#### `StreamConfig` enum

New tagged enum on `HttpGenericConfig`:

```rust
/// How the upstream endpoint streams chunks. Set on
/// [`HttpGenericConfig::stream_config`] to enable streaming.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamConfig {
    /// OpenAI-compatible SSE: `event:` + `data: <json>` lines, `[DONE]`
    /// sentinel, JSON frame shape compatible with
    /// `openai::stream::OaStreamFrame`.
    OpenAiSse {
        /// JSON Pointer (RFC 6901) into each parsed frame for the
        /// content delta string. Defaults to `/choices/0/delta/content`.
        #[serde(default = "default_oa_content_pointer")]
        content_pointer: String,
        /// JSON Pointer to the per-frame finish reason. Defaults to
        /// `/choices/0/finish_reason`.
        #[serde(default = "default_oa_finish_pointer")]
        finish_reason_pointer: String,
        /// Optional JSON Pointer to a per-frame usage object. When
        /// resolvable, the `input_tokens` / `output_tokens` are merged
        /// into the final `ChatChunk::End`.
        #[serde(default)]
        usage_pointer: Option<String>,
    },
    /// Newline-delimited JSON: one full frame per line. No `event:`
    /// framing. Termination on EOF or on a frame whose
    /// `finish_reason_pointer` resolves to a non-null string.
    NdJson {
        content_pointer: String,
        finish_reason_pointer: String,
        #[serde(default)]
        usage_pointer: Option<String>,
    },
}
```

Operator-config example (placed in the rustdoc on `HttpGenericConfig`):

```rust
let cfg = HttpGenericConfig {
    // … the existing fields …
    stream_config: Some(StreamConfig::OpenAiSse {
        content_pointer: "/choices/0/delta/content".into(),
        finish_reason_pointer: "/choices/0/finish_reason".into(),
        usage_pointer: Some("/usage".into()),
    }),
    ..Default::default()
};
```

Add `pub stream_config: Option<StreamConfig>` to `HttpGenericConfig` ([http-generic/src/lib.rs:47–67](/Users/kwc/tako-ai-core/crates/tako-providers/http-generic/src/lib.rs#L47-L67)) and to the `Default` impl as `None`.

#### Capability flag

In [HttpGenericProvider::new](/Users/kwc/tako-ai-core/crates/tako-providers/http-generic/src/lib.rs#L97), the `Capabilities` block at line 126 currently hardcodes `supports_streaming: false`. Phase 11.B sets it to `config.stream_config.is_some()` *unless* the operator explicitly overrode `config.capabilities`. Operator overrides win.

#### `stream()` impl

Replace the stub at [lib.rs:253–261](/Users/kwc/tako-ai-core/crates/tako-providers/http-generic/src/lib.rs#L253-L261). High-level flow:

1. Match on `self.inner.config.stream_config`. `None` returns `TakoError::Invalid("…no stream_config set on this provider")` — the operator clearly didn't intend to stream. Reword from the v0.11.0 marker text.
2. Render the body template (same `render_template` call used in `chat`).
3. POST and inspect status (same error mapping as `chat`).
4. Hand the body off to a per-variant adapter:
   - `StreamConfig::OpenAiSse { … }` → wrap `resp.bytes_stream()` in `Eventsource`, mirror the loop at [openai/src/stream.rs:70–135](/Users/kwc/tako-ai-core/crates/tako-providers/openai/src/stream.rs#L70-L135). For each event, resolve `content_pointer` + `finish_reason_pointer` + `usage_pointer` against the parsed frame `serde_json::Value`. Emit `ChatChunk::Delta { text: Some(s), tool_calls: vec![] }` for non-empty content. Always terminate with one `ChatChunk::End { finish_reason, usage }`.
   - `StreamConfig::NdJson { … }` → wrap `resp.bytes_stream()` in `tokio_util::codec::FramedRead<_, LinesCodec>` (or equivalent line splitter; `tokio-stream` already in workspace). One JSON parse per line; same pointer resolution; EOF terminates.

The pointer-resolution helper is shared:

```rust
fn resolve_str(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(|v| v.as_str()).map(str::to_string)
}
fn resolve_finish(value: &serde_json::Value, pointer: &str) -> Option<FinishReason> { … }
fn resolve_usage(value: &serde_json::Value, pointer: &str) -> Option<Usage> { … }
```

`tool_calls` are not extracted from streaming frames in Phase 11 — operators streaming tool-calls through http-generic must use the OpenAI provider's typed parser. Document this in the rustdoc.

#### `JsonPointer` reuse

`serde_json::Value::pointer` is RFC 6901-compliant out of the box; no new dependency. The `response_text_pointer` field on the existing single-shot `chat()` already uses this exact mechanism ([lib.rs:60–62](/Users/kwc/tako-ai-core/crates/tako-providers/http-generic/src/lib.rs#L60-L62)) — the streaming path is consistent with the single-shot path's contract.

#### Test list — 11.B

**`crates/tako-providers/http-generic/src/lib.rs::tests` (additions to the existing `mod tests`):**

- `stream_config_serialises_openai_sse_round_trip` — `serde_json::to_value(StreamConfig::OpenAiSse { … })` round-trips. Schema sanity.
- `stream_config_serialises_ndjson_round_trip` — same for `NdJson`.
- `default_pointers_match_openai_layout` — assert `default_oa_content_pointer() == "/choices/0/delta/content"`, etc.
- `capability_flag_set_when_stream_config_is_some` — build a `HttpGenericProvider` with `stream_config: Some(OpenAiSse { … })`, default `capabilities: None`; assert `provider.capabilities().supports_streaming == true`.
- `capability_flag_unset_when_stream_config_is_none` — provider built without `stream_config`; assert `supports_streaming == false`.
- `operator_capability_override_wins` — `stream_config: None`, `capabilities: Some(Capabilities { supports_streaming: true, … })`; assert `supports_streaming == true` (operator wins).

**`crates/tako-providers/http-generic/tests/streaming.rs` (new file, integration-style with `wiremock`):**

- `openai_sse_two_deltas_then_done_yields_delta_delta_end` — `wiremock` server emits `data: {"choices":[{"delta":{"content":"hel"}}]}\n\ndata: {"choices":[{"delta":{"content":"lo"}}]}\n\ndata: [DONE]\n\n`; assert exactly `[Delta("hel"), Delta("lo"), End { finish_reason: Stop }]`.
- `openai_sse_finish_reason_extracted` — final non-`[DONE]` frame includes `"finish_reason":"length"`; assert `End { finish_reason: Length, … }`.
- `openai_sse_usage_pointer_resolved` — final frame includes `"usage":{"input_tokens":12,"output_tokens":7}`; assert `End.usage == Usage { input_tokens: 12, output_tokens: 7 }`.
- `openai_sse_invalid_frame_yields_error_chunk` — emit `data: not-json\n\n`; assert `ChatChunk::Error { … }` is yielded but the stream continues to the next frame.
- `ndjson_two_lines_then_eof_yields_delta_delta_end` — server emits `{"text":"foo"}\n{"text":"bar","done":true}\n`; with `content_pointer: "/text"`, `finish_reason_pointer: "/done"`; assert `[Delta("foo"), Delta("bar"), End]`.
- `ndjson_terminates_on_eof_without_finish_reason` — server closes connection mid-stream; assert exactly one `End { finish_reason: Other, … }` is appended.
- `stream_without_stream_config_returns_invalid_error` — provider with `stream_config: None`; calling `.stream(…)` returns `Err(TakoError::Invalid(msg))` whose message contains "stream_config".
- `non_2xx_streaming_response_returns_provider_error` — `wiremock` 503; assert `TakoError::Provider` with status 503 and the body propagated.
- `stream_method_does_not_panic_when_pointer_is_unresolvable` — bad `content_pointer`; assert the stream emits zero `Delta`s plus one terminal `End` without panicking.

**`tests/python/test_phase11_http_generic_streaming.py`** — Python smoke. Build a `tako.providers.HttpGenericProvider` with `stream_config={"kind": "openai_sse", …}` against a fake server (e.g. `pytest-httpserver` or a tiny `aiohttp` fixture); call `SingleAgent.stream(…)`; assert the iterator yields the expected delta texts and one terminal `end` with usage. Mirrors the structure of [tests/python/test_phase10_python_streaming.py](/Users/kwc/tako-ai-core/tests/python/test_phase10_python_streaming.py).

**Public API risk:** additive — new optional `stream_config` field on `HttpGenericConfig`; new `StreamConfig` enum; existing `chat()` path unchanged. Without `stream_config: Some(…)`, behaviour is byte-for-byte identical to v0.11.0 (including the `Err(...)` from `.stream(…)`, with a slightly reworded message).

### 11.C — Examples + CHANGELOG + final flip

- New `examples/27_http_generic_streaming.py` — `HttpGenericProvider` with `stream_config={"kind": "openai_sse", "content_pointer": "/choices/0/delta/content"}` plugged into `SingleAgent`; prints token-by-token output.
- New `examples/28_state_store_hardened.py` — `JsonStateStore` round-trip showing the hardened path: explicit `umask 077`, `JsonStateStore.seed → verify → persist`, post-write `os.stat` proving `0o600` on Unix. Compatible-superset of `examples/23_state_store.py`.
- `CHANGELOG.md` — new `## [0.12.0]` block summarising 11.A and 11.B under `### Added`, `### Changed`, and `### Security` (the latter holds the H1/H2/M1–M4 line items so downstream operators can grep for `Security:`). Compare-link appended at bottom.
- `README.md` feature matrix: append a Phase 11 column with checks for "Sigstore hardening" and "http-generic streaming".
- `README.md` Roadmap section: append a Phase 11 bullet.
- `PLAN.md` phase-index table: flip Phase 11 to `done (date)`. Update "Phase 11 candidates" → "Phase 12 candidates", carrying forward the deferred items (see *Phase 12 candidates* below).
- Strike the `http-generic` streaming bullet from `PLAN.md → Backlog (uncommitted) → Stale phase markers — promised but not delivered` and tick the box.

## Critical files

| File | Phase 11 part | Change |
|------|---------------|--------|
| `crates/tako-governance/src/sigstore.rs:617–625` | 11.A H1 | `compare_exchange_weak` loop |
| `crates/tako-governance/src/sigstore.rs:516–524` | 11.A H1 | `Acquire`/`Release` ordering |
| `crates/tako-governance/src/sigstore.rs:680–701` | 11.A L2 | Iterate all SANs |
| `crates/tako-governance/src/sigstore.rs:709–717` | 11.A L5 | Doc comment only |
| `crates/tako-governance/src/sigstore.rs:829–887` | 11.A M3 | `BasicConstraints` + critical-ext check |
| `crates/tako-governance/src/sigstore.rs:928–942` | 11.A L3 | `BTreeMap`-based canonical SET |
| `crates/tako-governance/src/sigstore_state.rs:55–58` | 11.A M2 | `deny_unknown_fields` + `version` |
| `crates/tako-governance/src/sigstore_state.rs:99–127` | 11.A H2 | `0o600` chmod after rename |
| `crates/tako-governance/src/sigstore_state.rs:146–153` | 11.A M1/M4 | `NamedTempFile::new_in` |
| `crates/tako-governance/Cargo.toml:65` | 11.A M1 | Promote `tempfile` to `[dependencies]` |
| `crates/tako-governance/tests/sigstore.rs` | 11.A | New `mod hardening` (6 tests + L4) |
| `crates/tako-governance/tests/sigstore_state.rs` (new) | 11.A | New tests file (6 tests) |
| `python/tako/sigstore.py:230–257` | 11.A H2 | Docstring update |
| `crates/tako-providers/http-generic/src/lib.rs:47–67` | 11.B | `stream_config` field on `HttpGenericConfig` |
| `crates/tako-providers/http-generic/src/lib.rs:126–134` | 11.B | Cap-flag derives from `stream_config` |
| `crates/tako-providers/http-generic/src/lib.rs:253–261` | 11.B | Real `stream()` impl |
| `crates/tako-providers/http-generic/tests/streaming.rs` (new) | 11.B | New integration tests (9 tests) |
| `python/tako/providers.py` | 11.B | `stream_config=` kwarg forward |
| `crates/tako-py/python/tako/_native.pyi` | 11.B | `StreamConfig` stub |
| `Cargo.toml` (workspace + per-crate) | 11.0 | `0.11.0` → `0.12.0` |
| `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py` | 11.0 | Version bump |
| `examples/23_state_store.py` | 11.A H2 | One-line `umask 077` comment |
| `examples/27_http_generic_streaming.py` (new) | 11.C | New example |
| `examples/28_state_store_hardened.py` (new) | 11.C | New example |
| `CHANGELOG.md` | 11.C | New `## [0.12.0]` block |
| `README.md` | 11.C | Feature matrix + Roadmap row |
| `PLAN.md` | 11.0 / 11.C | Index in/out, backlog tick |
| `PLAN_PHASE11.md` (new) | 11.0 | Per-phase plan |

## Reused utilities (avoid re-inventing)

- `KeylessVerifier::with_rekor_min_tree_size` / `set_rekor_min_tree_size` / `rekor_max_tree_size` ([sigstore.rs:505–524](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore.rs#L505-L524)) — 11.A H1 only changes the internal ordering; the public surface stays.
- `JsonStateStore::save` / `seed` / `persist` ([sigstore_state.rs:60–144](/Users/kwc/tako-ai-core/crates/tako-governance/src/sigstore_state.rs#L60-L144)) — 11.A H2/M1/M2 swap implementation under existing function bodies, no signature changes.
- `tempfile = "3"` already at [crates/tako-governance/Cargo.toml:65](/Users/kwc/tako-ai-core/crates/tako-governance/Cargo.toml#L65) (dev-dep) — promote, do not re-add at a different version.
- `rcgen = "0.14"` already a dev-dep ([crates/tako-governance/Cargo.toml:63](/Users/kwc/tako-ai-core/crates/tako-governance/Cargo.toml#L63)) — reuse for the M3 chain-construction tests; do not add a second cert builder.
- `eventsource_stream::Eventsource` ([crates/tako-providers/openai/src/stream.rs:3](/Users/kwc/tako-ai-core/crates/tako-providers/openai/src/stream.rs#L3)) — already a workspace dep; 11.B's `OpenAiSse` variant uses the exact same call site shape.
- `async_stream::stream!` macro pattern ([openai/src/stream.rs:70–133](/Users/kwc/tako-ai-core/crates/tako-providers/openai/src/stream.rs#L70-L133)) — 11.B copies the loop body verbatim and swaps the pointer-based extraction in.
- `wiremock` already in `[dev-dependencies]` of every provider crate — 11.B reuses without a new dep.
- `serde_json::Value::pointer` (RFC 6901) — already used at [http-generic/src/lib.rs:60–62](/Users/kwc/tako-ai-core/crates/tako-providers/http-generic/src/lib.rs#L60-L62) in the single-shot `chat` path.
- `LinesCodec` from `tokio-util` — workspace dep; covers 11.B's NDJSON line splitter.

## Verification (Definition of Done)

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p tako-governance --features "sigstore sigstore-protobuf"   # 11.A
cargo test -p tako-providers-http-generic                                # 11.B

# Python
maturin develop --release --features "sigstore sigstore-protobuf"
pytest -q tests/python                                                   # +2 smoke tests, all green
ruff check python/ tests/python/ examples/                               # clean
ruff format --check python/ tests/python/ examples/                      # clean
mypy python/tako                                                         # clean

python -c "import tako; print(tako.__version__)"                         # → 0.12.0

# Examples (smoke-run, none require network or real keys)
python examples/27_http_generic_streaming.py     # against a localhost wiremock
python examples/28_state_store_hardened.py
```

## Acceptance gates

- **H1.** `multi_threaded_advance_never_observes_rollback` passes 100/100 runs under `cargo test --release -p tako-governance --features sigstore -- --test-threads=16 --nocapture` (release matters: H1 is observable only with reordered loads).
- **H2.** On Linux + macOS, `metadata().permissions().mode() & 0o777 == 0o600` after `JsonStateStore::save`. The Python smoke at `tests/python/test_phase11_state_store_hardening.py` asserts the same.
- **M1.** Eight concurrent `save()` calls on a shared `Arc<JsonStateStore>` all succeed; no orphan `*.tmp` files; final `load()` returns one of the eight written values.
- **M2.** A state file with an extra `attacker_field` is rejected (test passes); a v0.11.0 state file with no `version` field is loaded as v1 (test passes).
- **M3.** A chain through a non-CA "intermediate" is rejected; a chain with `pathLenConstraint` violation is rejected; an unknown-critical-extension chain is rejected.
- **M4.** Subsumed by M1; no separate gate.
- **L2.** Multi-SAN cert with one matching SAN is accepted; multi-SAN cert with zero matching SANs is rejected.
- **L3.** New canonical SET form is byte-equal to the v0.11.0 form for every existing fixture (regression).
- **L4.** New regression test exists and is part of the standard `cargo test` run.
- **11.B.** `HttpGenericProvider` with `stream_config: Some(OpenAiSse { … })` plugged into `SingleAgent.stream(…)` against a wiremock server emitting OpenAI-format SSE yields exactly the expected `Delta` sequence followed by exactly one `End { usage }`. Same for `NdJson`. Without `stream_config`, `.stream(…)` returns the v0.11.0-style invalid-config error.
- **CHANGELOG.** `## [0.12.0]` block added with `### Added` / `### Changed` / `### Security` sections; version bumped in all four locations; compare-link appended.
- **PLAN.** `PLAN_PHASE11.md` written; `PLAN.md` index flipped to `Phase 11 — done (date)`; "Phase 12 candidates" carries forward the deferred items; the `http-generic` streaming line in `PLAN.md → Backlog → Stale phase markers — promised but not delivered` is ticked.
- **README.** Feature matrix shows a Phase 11 column populated; Roadmap enumerates a Phase 11 bullet.

## Out of scope (intentional, with rationale)

- **Vision / image content support across providers (Anthropic, Vertex, Bedrock).** Multi-crate cross-cutting effort; the `ContentPart` enum needs a new variant and every provider's request-builder needs the corresponding wire-format mapping. Warrants its own phase. (Same call as Phase 10.)
- **Eval harness real graders (SWE-Bench Lite, GPQA Diamond).** Real SWE-Bench needs a sandboxed repo-test runner — a meaningful infra build-out in its own right. Standalone effort.
- **MCP Streamable HTTP — SSE upgrade + `Mcp-Session-Id` lifecycle.** Promised since Phase 2 and called out in PLAN.md's backlog. Protocol-spec implementation that warrants its own phase (per Phase 10's out-of-scope note). Don't bundle with provider streaming because the lifecycle semantics are entirely different.
- **Redis-backed `StateStore`.** Sibling to `JsonStateStore` for multi-replica deployments. Requires introducing a new `StateStore` trait so `JsonStateStore` and a future `RedisStateStore` can share the verifier-seeding API. That trait design is non-trivial (sync vs. async surface, error model, optional cross-replica lock semantics) and the operator hand-roll case is well-served by `JsonStateStore` today. Defer.
- **Streaming-aware verifier in Trinity / Conductor.** Phase 10.C emits `VerifierScore` only at synthesis-complete boundaries. Per-delta verifier calls would need the same opt-in cost-control surface as `LlmJudgeGuard::with_streaming_min_chars`. No consumer asks for it yet.
- **`tako-compat` real auth providers (Vault / JWT / OIDC).** Stale Phase 2 marker at [crates/tako-compat/src/auth.rs:5](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth.rs#L5). Belongs to a security/auth-focused phase, not bundled with sigstore hardening (different threat model, different reviewer audience).
- **HMAC sidecar on the state file.** SECURITY_PHASE10.md notes this is only worth it if H2 + a second compromise vector ever line up. The chmod fix closes the H2 attack-surface for typical deployments; defer the HMAC option until a concrete operator asks.
- **Streaming tool-call deltas for `http-generic`.** Operators streaming tool-calls through arbitrary endpoints have too many wire-format variants for a JSON-pointer extractor. Document the limitation in 11.B's rustdoc; redirect operators to the OpenAI provider's typed parser. Revisit only if a consumer asks.
- **`http-generic` "Custom" streaming variant.** A user-supplied closure for parsing the body would mean exposing a `Box<dyn Fn(Bytes) -> ChatChunk>` across the PyO3 boundary. Out of scope for v0.12.0; OpenAiSse + NdJson cover the surveyed-shape cases.
- **OTel end-to-end test against a real gRPC collector.** Phase 1.5 deferred item; orthogonal to this phase.
- **Vertex deterministic-per-call placeholder.** Stub revisit when usage warrants; no Phase 11 nexus.

## Phase 12 candidates (carry-forward)

Updated from `PLAN.md` → "Phase 11 candidates" at end of Phase 11. Six items, ordered by indicative size:

- **MCP Streamable HTTP — SSE upgrade + `Mcp-Session-Id` lifecycle.** Phase 2 promise; transport at [crates/tako-mcp/src/transport/streamable_http.rs:154](/Users/kwc/tako-ai-core/crates/tako-mcp/src/transport/streamable_http.rs#L154) still yields an empty stream. Standalone protocol work.
- **Vision / image content support across providers (Anthropic, Vertex, Bedrock).** Stub markers at [anthropic/src/convert.rs:171](/Users/kwc/tako-ai-core/crates/tako-providers/anthropic/src/convert.rs#L171), [vertex/src/convert.rs:203](/Users/kwc/tako-ai-core/crates/tako-providers/vertex/src/convert.rs#L203), [bedrock/src/convert.rs:268](/Users/kwc/tako-ai-core/crates/tako-providers/bedrock/src/convert.rs#L268).
- **Eval harness real graders** (SWE-Bench Lite, GPQA Diamond) — sandboxed runner needed.
- **Redis-backed `StateStore`** — requires introducing a `StateStore` trait. Sibling to `JsonStateStore` for multi-replica deployments.
- **Streaming-aware verifier in Trinity / Conductor.** Per-delta opt-in mirroring `LlmJudgeGuard::with_streaming_min_chars`. Lands when a concrete consumer asks.
- **`tako-compat` real auth providers** — Vault / JWT / OIDC, beyond `StaticTokens`.
