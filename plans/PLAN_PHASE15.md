# PLAN — Phase 15 (Streaming-aware Verifier in AbMcts + tako-compat auth hardening)

## Context

Phase 14 (v0.15.0, 2026-04-30) shipped streaming-aware
[`Verifier::evaluate_streaming`](/Users/kwc/tako-ai-core/crates/tako-core/src/traits/verifier.rs#L73-L79)
wiring in
[`Conductor::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs#L535-L623)
(closing the symmetric gap left by Phase 13.B's Trinity wiring) and
the first three real `tako-compat` auth resolvers
([`JwtAuthResolver`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/jwt.rs),
[`OidcAuthResolver`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs),
[`VaultAuthResolver`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/vault.rs)).
The Phase 14 writeup
[explicitly deferred three items](/Users/kwc/tako-ai-core/PLAN_PHASE14.md#L350-L362)
that are the natural Phase 15 bundle:

- Per-delta streaming `Verifier` in **AB-MCTS** rollouts (deferred
  from 14.A — "would require deeper refactor of the rollout sampler").
- **Vault dynamic token rotation** (AppRole / Kubernetes auth methods)
  — `VaultAuthResolver` shipped with a static Vault token only.
- **OIDC token introspection (RFC 7662)** — `OidcAuthResolver` shipped
  with signature validation only; revoked tokens whose signature still
  verifies passed.

Phase 15 closes all three under v0.16.0. The structure mirrors Phase 14
exactly: A is the third leg of the streaming-verifier triumvirate
(Trinity in 13.B → Conductor in 14.A → AB-MCTS in 15.A); B is the
auth hardening that 14.B's rustdoc + out-of-scope section pointed at.

Both items are strictly additive — public APIs unchanged shape.

**Theme:** *Streaming-verifier parity across all three orchestrators
+ production-grade Vault/OIDC auth.*

**Target tag:** v0.16.0.

## A. Streaming-aware `Verifier` in `AbMcts::stream`

### What ships

#### A.1 — Streaming rollout helper

[crates/tako-orchestrator/src/ab_mcts.rs](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs)
gains a sibling `rollout_static_streaming` function modeled on
[`rollout_static`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L730-L850)
and the
[Trinity precedent](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/trinity.rs#L490-L596).
When the picked provider advertises
`Capabilities::supports_streaming`, every provider turn in the rollout
goes through `provider.stream(...)` — tool-call turns assemble
`ToolCallDelta`s into `ContentPart::ToolCall` exactly like Trinity, the
final turn produces only `ContentPart::Text`. A cumulative
`text` buffer spans the entire rollout; on each non-empty
`ChatChunk::Delta` the buffer grows and
`Verifier::evaluate_streaming(&principal, &cumulative_text)` is
called. `Ok(Some(score))` returns produce intermediate
`OrchEvent::VerifierScore { step, branch, score: score.clamp(0.0,
1.0) }` events on the same `(step, branch)` as the eventual
synthesis-complete final.

Stream-startup failure (`Err` from `provider.stream`) falls back to
`provider.chat()` for that turn, emitting one full-text
`OrchEvent::AssistantText` — mirroring the Trinity degraded path.

#### A.2 — Wire into `AbMcts::stream`

[`AbMcts::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L363-L551)
now branches on `picked.capabilities().supports_streaming` after
[`pick_rollout_provider`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L697-L723)
(Phase 9.D — router-driven branch expansion). The streaming branch
spawns the helper into a `Box::pin`'d future and drives a
`tokio::select!` recv-loop on an `mpsc::unbounded_channel<OrchEvent>`,
forwarding events out of the `try_stream!` block while the rollout
runs concurrently. Drains any buffered events on rollout completion
before yielding the final `OrchEvent::VerifierScore` from `score()`.

The non-streaming branch keeps calling
[`rollout_static`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs#L730-L850)
and emits one full-rollout-text `OrchEvent::AssistantText` —
byte-for-byte parity with v0.15.0.

**Branch identity invariant.** `let leaf_idx = nodes.len() as u32;` is
computed *before* the rollout starts (the leaf will land at exactly
that index because `nodes` isn't mutated during the rollout). Per-delta
partials and the synthesis-complete final share `(step, branch =
leaf_idx)`. `debug_assert_eq!(nodes.len(), leaf_idx)` catches any
future regression.

**Router-driven mode interaction (Phase 9.D).** Capability check fires
on the **picked** provider, not the primary. Mixed-capability candidate
pools are supported: a streaming primary + non-streaming candidate
selected by the router degrades gracefully, and vice versa.

#### A.3 — `RuleBasedVerifier` already inherits

[`RuleBasedVerifier`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/verifiers.rs)
overrode `evaluate_streaming` in Phase 13.B; AB-MCTS users get the
cheap heuristic per-delta hook for free. No change needed.

### Tests

`crates/tako-orchestrator/tests/ab_mcts_streaming_verifier.rs` (new):

- `ab_mcts_stream_emits_per_delta_assistant_text` — 3 deltas × 2
  rollouts → 6 `AssistantText` events in scripted order.
- `ab_mcts_stream_emits_per_delta_verifier_score` — `CountingStreamingVerifier`
  → 3 partials × 2 rollouts (0.5) + 2 finals (0.9) = 8 `VerifierScore` events.
- `ab_mcts_stream_partial_and_final_share_branch` — partials' branch
  matches their rollout's final's branch.
- `ab_mcts_stream_default_evaluate_streaming_no_partials` — `AlwaysScore`
  (default `Ok(None)`) yields zero partials, exactly one final per
  rollout (Phase 8 byte-parity).
- `ab_mcts_stream_non_streaming_fallback_byte_parity` — `FakeProvider`
  (`supports_streaming = false`) → one full-text `AssistantText` per
  rollout, identical to v0.15.0.
- `ab_mcts_stream_router_picks_streaming_candidate` — router picks
  streaming candidate over non-streaming primary; assert deltas arrive.
- `ab_mcts_stream_router_picks_non_streaming_candidate` — opposite case;
  single-shot behaviour preserved.

**No Python facade changes for 15.A.** `PyAbMcts.stream` (Phase 8.B)
already surfaces `OrchEvent` partials through `PyOrchEventStream`;
Phase 15.A just populates the existing pipe with new event types.

## B. tako-compat auth hardening

### B.1 — Vault dynamic token rotation

#### B.1.a — `VaultTokenProvider` trait + 3 impls

[crates/tako-compat/src/auth/vault_token.rs](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/vault_token.rs)
(new):

```rust
#[async_trait]
pub trait VaultTokenProvider: Send + Sync + 'static + Debug {
    async fn token(&self) -> Result<(String, Option<Duration>), TakoError>;
}
```

- `StaticVaultToken(String)` — wraps a fixed string. Lossless equivalent
  of v0.15.0 behaviour.
- `AppRoleTokenProvider { addr, role_id, secret_id, http: reqwest::Client,
  cache: Arc<RwLock<Option<CachedAuth>>> }` — POSTs `{role_id, secret_id}`
  to `<addr>/v1/auth/approle/login`, parses `auth.client_token` +
  `auth.lease_duration`, re-authenticates lazily at
  `0.9 * lease_duration` (`REFRESH_FRACTION`).
- `KubernetesTokenProvider { addr, role, jwt_path: PathBuf, http, cache }`
  — reads the SA JWT from `jwt_path` via `tokio::fs::read_to_string`
  per-auth (so SA-token rotation is picked up), POSTs `{role, jwt}` to
  `<addr>/v1/auth/kubernetes/login`. Constructor is infallible —
  missing-JWT errors surface only when `token()` is actually called,
  so unit tests on dev workstations work without a populated
  `/var/run/secrets/...`. Convenience constructor
  `KubernetesTokenProvider::in_pod(addr, role)` hardcodes
  `DEFAULT_KUBERNETES_JWT_PATH`.

All providers POST directly via `reqwest` (NOT
`vaultrs::auth::approle/auth::kubernetes`) so we don't bump the
`vaultrs 0.7` dep. Internal helper `vault_login(http, url, body)`
parses the standard Vault auth-response JSON shape.

#### B.1.b — Refactored `VaultAuthResolver`

[crates/tako-compat/src/auth/vault.rs](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/vault.rs):

- Replaces `client: Arc<VaultClient>` with
  `provider: Arc<dyn VaultTokenProvider>` + `addr: String` +
  `client_cache: Arc<RwLock<HashMap<String, Arc<VaultClient>>>>`
  (bounded 4 entries — token rotation depth in practice).
- Existing `pub fn new(addr, vault_token)` (v0.15.0 signature) keeps
  working — internally constructs `Arc::new(StaticVaultToken::new(...))`.
- New constructors:
  - `with_provider(addr, provider)` — generic.
  - `with_approle(addr, role_id, secret_id) -> Result<Self, _>`
  - `with_kubernetes(addr, role, jwt_path) -> Result<Self, _>`
  - `with_kubernetes_in_pod(addr, role) -> Result<Self, _>`
- `resolve()` flow: `provider.token().await` → `get_or_build_client`
  (cache lookup; on miss build a fresh `VaultClient` and insert,
  evicting LRU on overflow) → `vaultrs::kv2::read(...)`.
- The existing principal cache (`Arc<RwLock<HashMap<String, (Principal,
  Instant)>>>`, 60s TTL) is **orthogonal** to Vault-token rotation —
  documented in rustdoc to forestall confusion.

#### B.1.c — Cargo + re-exports

`vault = ["dep:vaultrs", "dep:reqwest"]` (added `reqwest` so the
direct-`reqwest` Vault providers compile). New public surface re-
exported from `tako_compat::auth::*` and `tako_compat::*`.

### B.2 — OIDC token introspection (RFC 7662)

[crates/tako-compat/src/auth/oidc.rs](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs):

- New public `IntrospectionConfig { introspect_uri, client_id,
  client_secret }` struct (Debug + Clone).
- `DiscoveryDoc` extended with `#[serde(default)]
  introspection_endpoint: Option<String>` — captured at
  `discover()` time into a new
  `discovered_introspection_uri: Option<String>` field.
- `OidcAuthResolver` derives `Clone` (the JWKS cache is
  `Arc<RwLock<...>>`, so cloning shares the cache — cheap and
  correct).
- New builders:
  - `with_introspection(client_id, client_secret) -> Result<Self,
    TakoError>` — uses the discovered URI; **fail-closed** if the
    issuer didn't advertise an `introspection_endpoint`. This is the
    load-bearing safety: silent degradation would let an operator
    believe revocation is enforced when it isn't.
  - `with_introspection_uri(uri, client_id, client_secret) -> Self` —
    explicit URI, infallible (bypasses discovery).
- New private `introspect(token: &str) -> Result<(), TakoError>` is
  called from `resolve()` after signature validation succeeds. POSTs
  `token=<jwt>&token_type_hint=access_token` as URL-encoded form data
  with HTTP Basic auth carrying `client_id:client_secret`. Workspace
  `reqwest` is built without the `urlencoded` feature, so the body is
  built via `url::form_urlencoded::Serializer` (added behind the
  `oidc` feature gate). Response with `active=false` returns
  `TakoError::Invalid("oidc: token revoked (introspection ...)")`.

### B.3 — Python facade

`crates/tako-py/src/py_compat.rs`:

- `PyOidcAuth.inner` and `PyVaultAuth.inner` now hold concrete
  `Arc<OidcAuthResolver>` / `Arc<VaultAuthResolver>` (rather than
  `Arc<dyn AuthResolver>`) so builder methods can clone-and-modify.
  The `serve_openai_py` extract-auth-resolver path coerces to
  `Arc<dyn AuthResolver>` at use time.
- New `PyVaultAuth` static methods:
  - `with_approle(addr, role_id, secret_id)`
  - `with_kubernetes(addr, role, jwt_path)`
  - `with_kubernetes_in_pod(addr, role)`
- New `PyOidcAuth` builder methods (immutable — return new pyclass
  instance):
  - `with_introspection(client_id, client_secret=None) -> PyResult<Self>`
  - `with_introspection_uri(uri, client_id, client_secret=None) -> Self`

`python/tako/compat.py` rustdoc updated to document the new methods.
No new top-level Python re-exports — methods are accessed via the
existing `compat.VaultAuth.with_*` / `compat.OidcAuth.with_*` chains.

### Tests

Rust:

- `crates/tako-compat/src/auth/vault_token.rs` `#[cfg(test)] mod
  tests` — `Send + Sync + 'static` smoke; `StaticVaultToken` returns
  fixed value; `AppRoleTokenProvider::new` does no I/O;
  `KubernetesTokenProvider::token` surfaces missing-path as
  `TakoError::Transport`.
- `crates/tako-compat/tests/vault_token.rs` (new) — 6 wiremock
  integration tests: static-token byte-parity, AppRole login response
  parsing + caching, AppRole 5xx propagation, Kubernetes JWT-from-
  file, Kubernetes missing-JWT path, AppRole re-auth after lease
  expiry.
- `crates/tako-compat/src/auth/oidc.rs` `#[cfg(test)] mod tests` — 8
  new tests: `IntrospectionConfig: Clone + Debug`,
  `with_introspection` errors when no endpoint advertised,
  `with_introspection_uri` bypasses discovery, `introspect()`
  active=true returns Ok, active=false returns Invalid("revoked"),
  Basic auth header carries `client_id:secret`, 5xx propagates,
  no-op when disabled, `DiscoveryDoc` parses optional
  `introspection_endpoint`.

Python:

- `tests/python/test_phase15_auth_hardening.py` (new) — 5 smoke
  tests: `VaultAuth.with_approle / with_kubernetes /
  with_kubernetes_in_pod` construct without contacting Vault;
  `OidcAuth.with_introspection / with_introspection_uri` exist on the
  pyclass; `VaultAuth.with_*` exist on the pyclass.

## Out of scope (deferred to Phase 16+)

- **Vision / image content support across providers** — Anthropic /
  Vertex / Bedrock cross-cutter (carry-forward).
- **Eval harness real graders** (SWE-Bench Lite, GPQA Diamond) —
  sandboxed runner needed (carry-forward).
- **OIDC `introspection_endpoint_auth_method` discovery** — Phase
  15.B.2 supports HTTP Basic only; mTLS and `client_secret_jwt`
  deferred.
- **OIDC refresh-token flows / end-session endpoint** — orthogonal.
- **Vault namespace support** (Vault Enterprise) — orthogonal.
- **Composite `AuthResolver`s** (mTLS + bearer chaining).
- **Bounded `mpsc` backpressure** for slow per-delta verifiers in
  AB-MCTS rollouts.
- **Per-tenant rate limiting** in compat — orthogonal.

## Risks / open design questions

1. **AB-MCTS rollout streaming changes provider call shape.** Tool-
   call assembly now goes through `ToolCallDelta`s rather than the
   non-streaming `ContentPart::ToolCall` mapping. Mitigated by lifting
   the assembly logic verbatim from Trinity's
   [`assemble_tool_calls_pub`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/single.rs#L743)
   path; covered by the existing AB-MCTS test suite (which already
   passes against the new code path) and the new streaming tests.
2. **Vault token cache eviction.** Bounded LRU with `CLIENT_CACHE_LIMIT
   = 4` is a heuristic — token rotation depth in practice. If a
   deployment rotates >4 tokens within their effective overlap window,
   `VaultClient` rebuilds will fire on misses. Documented; raise the
   limit or revisit if it bites.
3. **OIDC introspection HTTP timeout.** Currently 10s (inherited from
   the existing OIDC `Client::builder`). A slow introspection endpoint
   adds 10s to every `resolve()`. Doc-comment guidance: introspection
   should be cached at the issuer (Keycloak default: 30s).
4. **OIDC introspection silent-degradation footgun.**
   `with_introspection(...)` returns `Result<Self, _>` and errors when
   discovery has no `introspection_endpoint` AND no override URL is
   set. The fallible-builder pattern is unusual in tako but matches
   the security goal: an operator who *thinks* they enabled
   revocation must not be silently downgraded to signature-only.

## Incidental documentation flips

- [PLAN.md](/Users/kwc/tako-ai-core/PLAN.md) — add Phase 15 row;
  strike the three closed carry-forwards (per-delta AB-MCTS streaming
  verifier, Vault dynamic token rotation, OIDC token introspection).
- [README.md](/Users/kwc/tako-ai-core/README.md) — feature matrix
  Phase 15 column; roadmap entry for v0.16.0.
- [CHANGELOG.md](/Users/kwc/tako-ai-core/CHANGELOG.md) — `## [0.16.0]`
  section mirroring the 0.15.0 structure.
- [PLAN_PHASE14.md](/Users/kwc/tako-ai-core/PLAN_PHASE14.md) — annotate
  the three deferred items with "(closed in Phase 15)".
- Version bumps: workspace `Cargo.toml` → `0.16.0`,
  `pyproject.toml`, `python/tako/__init__.py::__version__`,
  `tests/python/test_smoke.py` version assert.
