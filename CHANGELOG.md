# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

(none)

## [0.48.0] - 2026-05-02

Phase 47 — closes the **"OTel end-to-end test against a real
gRPC collector"** carry-forward item from
[PLAN.md](PLAN.md), originally deferred from the Phase 1.5
acceptance criteria. Until now no test asserted that spans
actually arrive at a collector — only that
[`init_otlp_tracing`](crates/tako-governance/src/otel.rs)
doesn't error and the orchestrator keeps running while
attached. A regression in span emission (e.g. a breaking
change in `tracing-opentelemetry` causing spans to silently
drop) wouldn't have been caught by any test. Phase 47 fixes
that with a self-contained `tonic`-based mock collector — no
external `otelcol-contrib` binary, no Docker.
Plan: [plans/PLAN_PHASE47.md](plans/PLAN_PHASE47.md).

### Tests

- **`crates/tako-governance/tests/otlp_collector_e2e.rs`** —
  spawns an in-process `tonic` server implementing
  `opentelemetry_proto::tonic::collector::trace::v1::TraceService::Export`
  on `127.0.0.1:0`. Every received `ResourceSpans` is buffered
  in a shared `Mutex<Vec<_>>`. The test:
    1. Calls `init_otlp_tracing` pointed at the mock's
       endpoint.
    2. Emits a marker span via
       `tracing::info_span!("phase47_marker_span", marker = "phase47-e2e")`.
    3. Drops the `TracerGuard` to flush the
       `BatchSpanProcessor`.
    4. Asserts the span name reached the mock with the
       `marker` attribute intact, plus the `service.name=tako`
       and `service.version=<crate version>` resource
       attributes that `init_otlp_tracing` always sets.

  This proves the **full path**: `tracing` → `tracing-opentelemetry`
  layer → `opentelemetry-otlp` exporter → `tonic` client →
  wire → `tonic` server → assertion.

### Docs

- [`tests/python/test_otlp.py`](tests/python/test_otlp.py) —
  removed the "Phase 2 deferred" caveat and pointed at the
  new Rust e2e file. The Python test continues to cover the
  lifecycle / facade contract (init → run → re-init
  rejected → shutdown idempotent); span content is now
  asserted on the Rust side where the wire path runs.

### Dev-deps

- `opentelemetry-proto` 0.31 (`gen-tonic` + `gen-tonic-messages`
  + `trace`) — gRPC server traits for the mock. Already a
  transitive dep via `opentelemetry-otlp`, so this only
  widens visibility.
- `tonic` (workspace; promoted from production-only on other
  crates to dev-dep on `tako-governance`).
- `tokio-stream` (workspace, `net` feature) for
  `TcpListenerStream` glue between `tokio` listener and
  `tonic::transport::Server::serve_with_incoming`.

No runtime dep changes.

## [0.47.0] - 2026-05-02

Phase 46 — Phase-1 placeholder sweep. Three independent
small cleanups identified in the post-Phase-45 tech-debt
review: a stale module docstring, an under-populated Python
result wrapper, and a non-stable Vertex tool-call ID
scheme. Closes the "Vertex deterministic-per-call placeholder
logic" carry-forward item from
[PLAN.md](PLAN.md). Plan: [plans/PLAN_PHASE46.md](plans/PLAN_PHASE46.md).

### Added

- **`tako._native.OrchOutput`** — new pyclass returned from
  every orchestrator's `run` / `run_sync`. Exposes `text`
  (unchanged from Phase 1; field name remains stable),
  `input_tokens`, `output_tokens`, `total_tokens`, and
  `steps` getters. The Rust `OrchOutput::message` field
  stays internal — exposing it cleanly needs `ContentPart` /
  `Message` round-tripping machinery; defer until ask.
- **`tako.SingleAgent` / `Conductor` / `SelfCaller` / `AbMcts` /
  `Trinity` results** now carry `usage` (a Pydantic
  [`Usage`](python/tako/models.py)) and `steps`
  (`int`, number of provider calls). The Python `_Result`
  wrapper swapped from a `text`-only placeholder
  ([python/tako/orchestrator.py](python/tako/orchestrator.py))
  to a thin façade over the new `OrchOutput` pyclass —
  `result.text` keeps working unchanged for existing
  callers.

### Fixed

- **Vertex tool-call IDs are now stable** across re-parses,
  retries, and serialisation round-trips
  ([crates/tako-providers/vertex/src/convert.rs](crates/tako-providers/vertex/src/convert.rs)).
  Previously the non-streaming response parser used the
  position-derived placeholder `vertex_call_<n>` where `n`
  was the current `content` vector length — IDs flipped
  whenever intervening text content reordered. Replaced
  with `SipHash13((name, args-as-canonical-JSON))` rendered
  as `vertex_call_<16-hex-chars>`. Same name + same args →
  same id; different name OR different args → different id.
  Two new unit tests (`vertex_call_id_is_stable_across_reparses`,
  `vertex_call_id_distinguishes_different_calls`) pin the
  contract. **Note**: the streaming path
  ([crates/tako-providers/vertex/src/stream.rs](crates/tako-providers/vertex/src/stream.rs))
  still uses per-stream `tool_call_index` for within-stream
  chunk reassembly — that's a different concern (chunk
  identity vs cross-call identity) and is out of scope here.
- **Stale `harness.py` docstring**
  ([python/tako/eval/harness.py](python/tako/eval/harness.py))
  — claimed `swe_bench_lite` and `gpqa_diamond` raise
  `NotImplementedError`. They don't; Phase 4 wired up
  on-demand HuggingFace loaders. Docstring now accurately
  describes the loaders + their lightweight verifiers, and
  flags real SWE-Bench grading (apply patch + sandboxed
  tests) as future work.

### Tests

- **`tests/python/test_phase46_orch_output_fields.py`** —
  4 new tests covering `result.text` / `usage` / `steps`
  exposure on `SingleAgent` (async + sync) plus a `repr`
  assertion. Use `FakeProvider`; no API keys needed.
- **2 new lib tests** in
  [crates/tako-providers/vertex/src/convert.rs](crates/tako-providers/vertex/src/convert.rs)
  for the Vertex tool-call ID contract.

### Internal

- New module
  [crates/tako-py/src/py_orch_output.rs](crates/tako-py/src/py_orch_output.rs)
  hosts the `PyOrchOutput` pyclass. Registered in
  `_native` (always-on; no feature gate). Five orchestrator
  pyclasses (`PyOrchestrator`, `PyConductor`, `PySelfCaller`,
  `PyAbMcts`, `PyTrinity`) updated to return `PyOrchOutput`
  from `run` / `run_sync` instead of `String`. The wire
  contract for `result.text` is unchanged.

## [0.46.0] - 2026-05-02

Phase 45 — closes the Python facade gap on the Phase 44
operator-supplied-CA discovery constructor. Phase 44 shipped
the Rust API + 6 wire-level tests; Python wheel operators
behind a private internal CA now reach the same surface
without a custom `reqwest::Client` route. Closes the Phase 44
follow-up identified in
[plans/PLAN_PHASE44.md](plans/PLAN_PHASE44.md).
Plan: [plans/PLAN_PHASE45.md](plans/PLAN_PHASE45.md).

### Added

- **`OidcAuth.discover_with_extra_root(issuer, audience, extra_root_ca_pem)`**
  — Python staticmethod sibling of the Phase 44
  [`OidcAuthResolver::discover_with_extra_root`](crates/tako-compat/src/auth/oidc.rs)
  Rust constructor. Async — returns a coroutine. Builds the
  resolver-wide HTTP client with the operator-supplied
  PEM-encoded root CA bundle (single cert or concatenated
  multi-cert PEM) added to its trust store. The same trust
  anchor covers BOTH the OIDC discovery doc fetch (during
  construction) AND every subsequent JWKS refresh, because the
  resolver holds a single HTTP client for non-introspection
  HTTP. PEM parse failures (empty bundle, garbage bytes) raise
  `ValueError` at construction time — fail-closed. Pair with
  the Phase 43 `with_introspection_mtls_extra_root` Python
  builder when one PKI fronts the whole OIDC stack. Gated on
  the `auth-oidc` cargo feature (same as the parent
  `OidcAuth` pyclass).

### Tests

- **`tests/python/test_phase45_discover_extra_root_python.py`** —
  three new smoke tests pin the Python-side surface (binding
  name + staticmethod nature + awaitable return) so a
  regression in the PyO3 wrapping lands here before user code.
  The wire-level / PEM-parse / persistence semantics are
  covered by the Rust unit + integration tests in
  [`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs)
  and
  [`crates/tako-compat/tests/oidc_mtls_e2e.rs`](crates/tako-compat/tests/oidc_mtls_e2e.rs).

### Docs

- `python/tako/compat.py` — appended a Phase 44 paragraph to
  the `serve_openai` running docstring documenting the new
  constructor alongside the Phase 42 introspection-mTLS prose.

## [0.45.0] - 2026-05-02

Phase 44 — closes the "Custom CA support for non-introspection
endpoints (JWKS, discovery)" carry-forward from Phase 42's
out-of-scope section. Phases 42 + 43 covered the operator-
supplied-CA story for OIDC introspection (the mTLS POST), but
the resolver-wide HTTP client used by `discover()` for the
discovery doc fetch + by the JWKS refresh path had no CA
injection point. Operators behind a private internal CA
couldn't even **boot** `OidcAuthResolver::discover` against
their issuer — the discovery GET failed TLS verification
before the resolver was returned. Phase 44 adds a parallel
constructor; the rest of the discovery surface is unchanged.
Plan: [plans/PLAN_PHASE44.md](plans/PLAN_PHASE44.md).

### Added

- **`OidcAuthResolver::discover_with_extra_root(issuer, audience, extra_root_ca_pem)`**
  — parallel async constructor that builds the resolver-wide
  [`reqwest::Client`](crates/tako-compat/src/auth/oidc.rs)
  with an operator-supplied PEM-encoded root CA bundle added
  to its trust store. Same trust anchor covers BOTH the
  OIDC discovery doc fetch (during construction) AND every
  subsequent JWKS refresh, because the resolver holds a single
  `http` field for non-introspection HTTP. For enterprise
  self-hosted OIDC issuers (Keycloak / Auth0 self-hosted /
  Authentik) presenting a server cert signed by a private
  internal CA. Concatenated multi-cert PEM bundles (root +
  intermediates) work via `reqwest::Certificate::from_pem_bundle`.
  PEM parse failures (empty bundle, garbage bytes) raise
  `TakoError::Invalid` synchronously at construction time —
  fail-closed at the operator boundary, no runtime
  surprises. Independent from
  [`with_introspection_mtls_extra_root`](crates/tako-compat/src/auth/oidc.rs):
  the introspection mTLS client carries its own CA store;
  operators with one PKI for the whole stack pass the same
  PEM bundle to both.

### Tests

- **3 new lib tests** in
  [`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs)
  covering the constructor's fail-closed PEM-parse contract:
  garbage CA, empty bundle, and the
  PEM-passes-then-network-error path that proves a valid
  bundle clears construction and reaches the actual GET.
- **2 new e2e tests** in
  [`crates/tako-compat/tests/oidc_mtls_e2e.rs`](crates/tako-compat/tests/oidc_mtls_e2e.rs)
  exercising HTTPS discovery + JWKS against a per-test
  `axum-server` rooted at a private `rcgen` CA:
    - `discover_over_https_with_private_ca_succeeds` — full
      happy-path: `discover_with_extra_root` →
      [`resolve(token)`](crates/tako-compat/src/auth/oidc.rs)
      against the same private-CA issuer (proves the trust
      anchor flows through to the JWKS path too).
    - `discover_over_https_without_extra_root_fails` —
      default `discover()` against the same server →
      `TakoError::Transport` from the discovery GET (proves
      the gap this phase closes).
  Plus `discover_with_extra_root_unparseable_pem_errors_at_constructor_time`
  in the e2e file pins the fail-closed contract on the
  full-deps test binary.

### Internal

- New private helper `build_resolver_http_client(extra_root_ca_pem)`
  in [`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs)
  — extracted from the original `discover()` body so both
  constructors share one Client-construction code path.
  Mirrors the Phase 42 `build_mtls_reqwest_client` shape
  (without the client cert / `Identity`, since this client is
  plain TLS not mTLS).
- `OidcAuthResolver::discover()` is now a one-line wrapper
  over a private `discover_inner(...)` async function. No
  behavior change for the default (public-CA) case.

### Deferred

- **Python facade for `discover_with_extra_root`** — same
  Rust→Python cadence as Phases 42→43, 39→40, 37→38. Lands
  in Phase 45.

## [0.44.0] - 2026-05-02

Phase 43 — closes the Python facade gap on the Phase 42
operator-supplied-CA mTLS introspection builders. Phase 42
shipped the Rust API + wire-level integration tests; Python
wheel operators now reach the same surface without dropping
to a custom `reqwest::Client` route. Closes the Phase 42
follow-up identified in
[plans/PLAN_PHASE42.md](plans/PLAN_PHASE42.md).
Plan: [plans/PLAN_PHASE43.md](plans/PLAN_PHASE43.md).

### Added

- **`OidcAuth.with_introspection_mtls_extra_root(cert_pem,
  key_pem, extra_root_ca_pem)`** — Python sibling of the
  Phase 42
  [`OidcAuthResolver::with_introspection_mtls_extra_root`](crates/tako-compat/src/auth/oidc.rs)
  Rust builder. Loads a client cert + private key AND adds
  an operator-supplied PEM-encoded root CA bundle (single
  cert or concatenated multi-cert PEM) to the underlying
  HTTP client's trust store. The bundle is persisted on
  `IntrospectionConfig::extra_root_ca_pem` so Phase 33 / 35
  / 37 / 39 rotation surfaces re-apply the same trust
  anchors after a cert/key swap. PEM parse failures (empty
  bundle, garbage bytes) raise `ValueError` at builder
  time — fail-closed, no runtime surprises. For enterprise
  self-hosted OIDC issuers (Keycloak / Auth0 self-hosted /
  Authentik) presenting a server cert signed by a private
  internal CA. Returns a NEW `OidcAuth` (immutable builder).
  Gated on the `auth-oidc` cargo feature (same as the parent
  `OidcAuth` pyclass).
- **`OidcAuth.with_introspection_self_signed_mtls_extra_root(...)`**
  — RFC 8705 §2.2 sibling of the above. Identical wire
  shape, same CA persistence; only the auth-method enum
  variant differs (`SelfSignedTlsClientAuth`).

### Tests

- **`tests/python/test_phase43_mtls_extra_root_python.py`** —
  three new smoke tests pin the Python-side surface
  (binding name + arg shape + callability) so a regression
  in the PyO3 wrapping lands here before user code. The
  wire-level / PEM-parse / persistence semantics are
  covered by the Rust unit + integration tests in
  [`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs)
  and
  [`crates/tako-compat/tests/oidc_mtls_e2e.rs`](crates/tako-compat/tests/oidc_mtls_e2e.rs).

### Docs

- `python/tako/compat.py` — appended a Phase 42 paragraph to
  the `serve_openai` running docstring documenting the new
  builders alongside the Phase 24 / 25 / 33 mTLS prose.

## [0.43.0] - 2026-05-02

Phase 42 — closes the "OIDC mTLS end-to-end integration test"
backlog item from [PLAN.md](PLAN.md). Six prior phases shipped
the mTLS introspection surface (Phases 24, 25, 33, 35, 37, 39),
but every existing test was builder-level: PEM parsing,
auth-method enum routing, `Arc<MtlsClient>` construction. None
exercised an actual mTLS handshake against a server requiring
client cert auth. Phase 42 lifts the `tako-mcp` Phase 5.B mTLS
test pattern (`rcgen` + per-test CA + `rustls`-backed
`axum-server`) into `tako-compat` and runs the full
`OidcAuthResolver::resolve(token)` path on the wire.
Plan: [plans/PLAN_PHASE42.md](plans/PLAN_PHASE42.md).

### Added

- **`OidcAuthResolver::with_introspection_mtls_extra_root`** —
  identical to the existing
  [`with_introspection_mtls`](crates/tako-compat/src/auth/oidc.rs)
  builder but accepts an operator-supplied PEM-encoded root CA
  bundle that's added to the underlying `reqwest::Client`'s
  trust store. For enterprise self-hosted OIDC issuers
  (Keycloak / Auth0 self-hosted / Authentik) presenting a
  server cert signed by a private internal CA. Concatenated
  multi-cert PEM bundles (root + intermediates) work via
  `reqwest::Certificate::from_pem_bundle`. PEM parse failures
  surface as `TakoError::Invalid` at builder time — no runtime
  surprises. The CA bundle is persisted on
  `IntrospectionConfig::extra_root_ca_pem` so subsequent
  `reload_mtls_identity` calls (and the rotation surfaces in
  Phases 35 / 37 / 39 that route through it) re-apply the same
  trust roots when rebuilding the mTLS client after a cert/key
  swap.
- **`OidcAuthResolver::with_introspection_self_signed_mtls_extra_root`**
  — RFC 8705 §2.2 sibling of the above. Same wire shape, same
  CA-persistence, only the `auth_method` enum variant differs
  (`SelfSignedTlsClientAuth`).
- **`IntrospectionConfig::extra_root_ca_pem`** — public field
  (`Option<Arc<Vec<u8>>>`) holding the operator-supplied trust
  bundle. Visible to the rotation surfaces; `None` for the
  default (public-CA) case.

### Tests

- **`crates/tako-compat/tests/oidc_mtls_e2e.rs`** — five new
  integration tests exercising the full OIDC mTLS introspection
  flow on the wire:
    - `mtls_round_trip_succeeds_with_extra_root` — full
      happy-path: discovery → JWKS → JWT validation → mTLS POST
      `/introspect` → `active=true` → `Principal` returned.
    - `mtls_handshake_fails_when_extra_root_not_configured` —
      mTLS client built without the extra CA → server cert is
      not trusted → handshake aborts → `TakoError::Transport`.
    - `mtls_handshake_fails_when_client_cert_missing` —
      resolver wired through the **non-mTLS** path → server's
      `RequireAndVerifyClientCert` policy aborts the handshake
      → resolve fails before introspection succeeds.
    - `self_signed_mtls_extra_root_round_trip_succeeds` — same
      happy-path using the `_self_signed_` builder.
    - `extra_root_unparseable_pem_errors_at_builder_time` —
      garbage CA bytes fail-closed synchronously at builder
      time, not at first-request time.
- **6 new lib tests** in `crates/tako-compat/src/auth/oidc.rs`
  cover the `_extra_root` builder happy/error paths AND the
  rotation-preserves-extra-root invariant.

### Internal

- New private helper `build_mtls_reqwest_client(cert, key,
  extra_root)` in `oidc.rs` — extracted from the per-builder
  bodies so the four mTLS Client-construction code paths
  (`with_introspection_mtls`,
  `with_introspection_self_signed_mtls`, the two new
  `_extra_root` siblings, plus `reload_mtls_identity`) share
  one implementation. No behavior change for the default
  (public-CA) case.

### Dev-deps

- `rcgen` 0.14, `axum-server` 0.8 (`tls-rustls`), `rustls`
  0.23 (`aws_lc_rs`), `jsonwebtoken` 10.3 (matching the Phase
  41 main-dep pin), `rsa` 0.9, `base64` 0.22 — all under
  `tako-compat` `[dev-dependencies]`, gated to the `oidc`
  feature flag. No runtime dep changes. PEM parsing in the
  test uses `rustls::pki_types::pem::PemObject` (already a
  transitive dep via `rustls`) to avoid the unmaintained
  `rustls-pemfile` (RUSTSEC-2025-0134).

## [0.42.0] - 2026-05-02

Phase 41 — security fix: bump `jsonwebtoken` from 9.3 to 10.3
to clear the Type-Confusion advisory (GHSA-vfgw-wj55-mp36,
medium severity, potential authorization bypass). PR #32
attempted this bump previously but the breaking-change
handling was wrong — 10.x requires explicit selection of a
`CryptoProvider` and moves the PEM helpers behind the
`use_pem` feature. Phase 39 reverted to 9.3 to unblock;
Phase 41 finishes the migration properly.
Plan: [plans/PLAN_PHASE41.md](plans/PLAN_PHASE41.md).

### Security

- **`jsonwebtoken` 9.3 → 10.3** — closes
  [GHSA-vfgw-wj55-mp36](https://github.com/Keats/jsonwebtoken/security/advisories/GHSA-vfgw-wj55-mp36)
  (Type Confusion that leads to potential authorization
  bypass). No source-code changes — every existing call site
  (`jwt.rs:68,75`, `oidc.rs:213,225,238,2254`) keeps working
  verbatim under the 10.x API once `use_pem` is on. Pinned
  `default-features = false, features = ["rust_crypto", "use_pem"]`
  to keep the pure-Rust crypto stack (no OpenSSL / aws-lc-rs
  system-library dep).
- **`rustls-webpki` 0.101.x advisories** — three open
  dependabot alerts (RUSTSEC-2026-0098 / -0099 / -0104) all
  reach via `aws-smithy-http-client → rustls 0.21.12`. The
  current `Cargo.lock` already has `rustls-webpki 0.103.13`
  on the modern paths; the legacy 0.101.x line stays pinned
  by the AWS SDK. Tako's URL-allowlist + URL pre-fetch
  surface doesn't parse CRLs or use URI-based name
  constraints, so the affected code paths aren't reachable.
  Documented + ignored in [`.cargo/audit.toml`](.cargo/audit.toml)
  with re-evaluation triggers; dependabot alerts dismissed
  with the same rationale. Will clear when AWS SDK migrates
  to rustls 0.23+ (`awslabs/aws-sdk-rust#1295`).

## [0.41.0] - 2026-05-02

Phase 40 — Python facade for the Phase 39 ``MtlsRefreshHook``.
Closes the deferred Python facade carry-forward from Phase 39:
Python-wheel operators using ``OidcAuth.watch_mtls_files``
(Phase 35.B) or ``OidcAuth.watch_mtls_provider`` (Phase 38) now
get full parity with the Rust API for the auto-retry layer.
After Phase 40 the entire Phase 33 mTLS rotation surface is
feature-complete on both Rust and Python sides.
Plan: [plans/PLAN_PHASE40.md](plans/PLAN_PHASE40.md).

### Added

- **`tako-py`: ``tako.compat.MtlsRefreshHook``** pyclass —
  Clone-able wrapper around the Phase 39 Rust handle. Returned
  by `MtlsFsWatcher.refresh_hook()` and
  `MtlsProviderWatcher.refresh_hook()`; pair with
  `OidcAuth.with_mtls_refresh_hook(hook)`.
- **`tako-py`: `OidcAuth.with_mtls_refresh_hook(hook)`**
  Python method mirroring the Phase 39 Rust API. Returns a NEW
  `OidcAuth` (immutable builder; matches the `with_introspection_*`
  cadence).
- **`tako-py`: `MtlsFsWatcher.refresh_hook()`** /
  `MtlsProviderWatcher.refresh_hook()` Python methods — return
  a `MtlsRefreshHook` wired to the watcher's background task.
  Raise `ValueError` if the watcher has been shut down.

## [0.40.0] - 2026-05-02

Phase 39 — auto refresh-on-handshake-failure for OIDC mTLS.
Closes the last Phase 33 mTLS-rotation carry-forward (strategy
2-of-3 was deferred Phase 33; strategy 3 shipped Phase 35;
strategy 1 shipped Phase 37/38; this phase ships strategy 2).
After Phase 39 the Phase 33 rotation backlog is fully retired.
Plan: [plans/PLAN_PHASE39.md](plans/PLAN_PHASE39.md).

### Added

- **`tako-compat`: `MtlsRefreshHook`** — handle that triggers
  an out-of-band mTLS reload from a Phase 35 filesystem watcher
  or Phase 37 trait-based provider. Internally a one-shot RPC
  channel between the introspection POST retry layer and the
  watcher / provider's background task. Capped at 2s per
  trigger.
- **`tako-compat`: `OidcAuthResolver::with_mtls_refresh_hook(hook)`**
  builder. When wired AND mTLS introspection is configured AND
  the introspection POST hits a `TakoError::Transport`, the
  retry layer triggers the hook, awaits the reload, and
  re-sends the POST exactly once. Cycle-detection is
  structural: at most one retry per `introspect()` call.
- **`tako-compat`: `MtlsFsWatcher::refresh_hook()`** /
  **`MtlsProviderWatcher::refresh_hook()`** — return a
  `Clone`-able `MtlsRefreshHook` wired to the watcher's
  background task. The same hook can be wired into multiple
  resolvers if several mTLS-introspecting endpoints share one
  cert source.

### Changed

- `tako-compat`: the introspection-POST send path is factored
  into `OidcAuthResolver::introspect_send_once` so the retry
  layer can rebuild the request (reqwest `RequestBuilder` is
  consumed by `.send`) with a fresh `MtlsClient::current()`
  snapshot after a forced reload. Pure refactor — Phase
  24/25/33/34/35/37/38 byte-for-byte cadence preserved when no
  retry hook is wired.
- `tako-compat`: `oidc_mtls_watcher::do_reload` widened from
  `()` to `Result<(), TakoError>` so the refresh-hook arm can
  signal success / failure back to the caller via the
  `oneshot` reply.

### Fixed

- `jsonwebtoken` reverted from 10.3 (dependabot PR #32) back
  to 9.3. The 10.x bump dropped PEM-helper functions
  (`from_rsa_pem` / `from_ec_pem` / `from_ed_pem`) entirely;
  PR #32 landed without doing the API migration, leaving
  main's tests broken. Phase 39 restores green tests as a
  side effect; proper 10.x migration is a Phase 40+ candidate.

## [0.39.0] - 2026-05-02

Phase 38 — Python facade for the Phase 37 trait-based mTLS
identity provider. Closes the deferred Python facade carry-
forward from Phase 37: Python-wheel operators can now wrap an
``async def fetch() -> tuple[bytes, bytes] | dict`` in
``tako.compat.MtlsIdentityProvider(...)`` and pass it to
``OidcAuth.watch_mtls_provider(provider)``. After Phase 38,
HSM-backed keys, in-memory secret stores, SPIFFE Workload API
and AWS IAM Roles Anywhere are all expressible through the
wheel — full parity with the Rust API.
Plan: [plans/PLAN_PHASE38.md](plans/PLAN_PHASE38.md).

### Added

- **`tako-py`: `tako.compat.MtlsIdentityProvider`** pyclass
  wrapping a Python async callable. Bridges via
  `pyo3_async_runtimes::tokio::into_future` (same pattern as
  `PyPythonProvider` for the LlmProvider trait). Accepted
  return shapes: `(cert_pem, key_pem)` tuple of bytes, or
  `{"cert_pem": bytes, "key_pem": bytes}` dict. Other shapes
  raise `TakoError::Invalid` at refresh time with a
  diagnostic.
- **`tako-py`: `OidcAuth.watch_mtls_provider(provider)`**
  Python method mirroring the Phase 37 Rust API. Returns a
  `MtlsProviderWatcher` handle whose `Drop` impl /
  `shutdown()` / `__exit__` stops the background task
  cleanly.
- **`tako-py`: new wheel feature `auth-mtls-identity-provider`**
  forwarding to `tako-compat/mtls-identity-provider`. Implies
  `auth-oidc`. Default wheel is unchanged.

## [0.38.0] - 2026-05-02

Phase 37 — trait-based `MtlsIdentityProvider` for proactive
expiry-driven cert refresh. Carry-forward strategy (1-of-2-
remaining) from [plans/PLAN_PHASE33.md](plans/PLAN_PHASE33.md):
operators with non-filesystem mTLS rotation (HSM-backed keys,
in-memory secret stores, SPIFFE Workload API, AWS IAM Roles
Anywhere) now opt in to the new `mtls-identity-provider` cargo
feature, implement the `MtlsIdentityProvider` async trait, and
call `OidcAuthResolver::watch_mtls_provider(provider)` once at
startup. The returned `MtlsProviderWatcher` handle holds a
background tokio task that fetches a fresh identity, parses the
cert's `NotAfter`, and re-fetches at 80% of the validity window.
Plan: [plans/PLAN_PHASE37.md](plans/PLAN_PHASE37.md).

### Added

- **`tako-compat`: `MtlsIdentityProvider` async trait + proactive
  expiry-driven refresh.** New optional cargo feature
  `mtls-identity-provider` ships:
  - Public `MtlsIdentityProvider` async trait whose `fetch()`
    method yields a `MtlsIdentity { cert_pem, key_pem }`
    PEM-pair.
  - `OidcAuthResolver::watch_mtls_provider(provider)` builder
    spawning a background task that re-fetches at 80% of the
    cert's parsed validity window (clamped to `[60s, 24h]`).
  - `MtlsProviderWatcher` handle whose `Drop` impl /
    `shutdown().await` stops the task cleanly.
  - `x509-parser` dep (behind the new feature) for cert
    `NotAfter` parsing. Default `tako-compat` build is
    unchanged.
- **`MtlsClient::current()` widening from Phase 35** is reused
  by the Phase 37 tests for `Arc::as_ptr` snapshot comparison.

### Notes

- Python facade for `MtlsIdentityProvider` is **deferred** to
  Phase 38+ because of async-trait-from-Python ergonomics.
  Python-wheel operators continue to use the Phase 35
  filesystem watcher (`OidcAuth.watch_mtls_files`).
- Fetch errors retry on a 60s backoff; the previously installed
  Client stays in place per Phase 33 semantics. No bootstrap
  reload — the resolver requires a synchronous
  `with_introspection_mtls(...)` call before
  `watch_mtls_provider`, so the running server always has a
  valid identity before the first background fetch lands.

## [0.37.0] - 2026-05-02

Phase 36 — per-child `ChainedAuthResolver` short-circuit policy
override. Operators wiring composite auth chains can now mark
individual children as Inherit / AlwaysFallThrough /
TransportOnly / AllInfrastructure independent of the chain-wide
Phase 26 / 27 flag. Common pattern: chain-wide
`with_short_circuit_on_infrastructure_errors` plus a final
in-process static-tokens child marked `AlwaysFallThrough` so a
spurious infra error from the tail doesn't strand the chain.
Plan: [plans/PLAN_PHASE36.md](plans/PLAN_PHASE36.md).

### Added

- **`tako-compat`: per-child `ChainedAuthResolver` short-circuit
  policy override.** New public
  `tako_compat::ChildShortCircuitPolicy` enum
  (`Inherit` / `AlwaysFallThrough` / `TransportOnly` /
  `AllInfrastructure`) plus
  `ChainedAuthResolver::then_with_short_circuit(child, policy)`
  builder. Override priority: when a child's policy is anything
  other than `Inherit`, that policy alone determines whether the
  child's error halts the chain — the chain-wide flag is ignored
  for this child. Existing `then(child)` keeps Phase 21 cadence
  byte-for-byte (defaults to `Inherit`). Python facade:
  `ChainedAuth.then_with_short_circuit(child, policy)` accepts
  `"inherit"` / `"always_fall_through"` / `"transport_only"` /
  `"all_infrastructure"` (case-insensitive; kebab-case variants
  also accepted). Unknown policy strings raise `ValueError`
  listing the accepted aliases.

### Changed

- `docs/recipes/chained_auth.md` — adds a "Per-child policy
  override" section with the OIDC + JWT + static-tail mixed
  policy example.

## [0.36.0] - 2026-05-02

Phase 35 — OIDC mTLS filesystem-watcher auto-reload. Carry-forward
strategy (3) from [plans/PLAN_PHASE33.md](plans/PLAN_PHASE33.md):
operators using cert-manager / Vault PKI / SPIRE /
kubernetes-secret-mount rotation now opt in to the
`mtls-fs-watch` cargo feature (Python wheel feature
`auth-mtls-fs-watch`) and call
`OidcAuthResolver::watch_mtls_files(cert_path, key_path)` once at
startup. The returned `MtlsFsWatcher` handle holds a background
tokio task that re-reads + reloads the mTLS identity whenever
either file changes on disk, replacing the hand-rolled polling
loop the Phase 33 recipe suggested. Plan:
[plans/PLAN_PHASE35.md](plans/PLAN_PHASE35.md).

### Added

- **`tako-compat`: OIDC mTLS filesystem-watcher auto-reload** —
  new optional cargo feature `mtls-fs-watch` (Python wheel
  feature `auth-mtls-fs-watch`) ships an
  `OidcAuthResolver::watch_mtls_files(cert_path, key_path)`
  helper that wraps the [`notify`](https://docs.rs/notify) crate
  to auto-call `reload_mtls_identity` whenever the watched cert
  or key files change on disk. Behaviour: watches the *parent
  directories* (atomic-rename safe per cert-manager /
  kubernetes-secret-mount conventions), 500 ms debounce
  coalesces bursty writes, reload failures logged at `warn!`
  without killing the watcher (Phase 33's "previously installed
  Client preserved on parse error" guarantee covers transient
  mid-rotation invalid-PEM reads). Returns an `MtlsFsWatcher`
  handle whose `Drop` impl shuts the background task down
  cleanly. Python facade: `OidcAuth.watch_mtls_files(...)`
  returns a context-manager-friendly `MtlsFsWatcher`. Default
  `tako-compat` build is unchanged — feature is opt-in.
- **`MtlsClient::current()` is now `pub`** (was `pub(crate)`)
  so operator observability tooling and the new watcher
  integration tests can compare snapshots before/after a
  rotation via `Arc::as_ptr`. The inner Client is read-only;
  swap remains pub(crate).
- **`OidcAuthResolver::introspection_mtls_configured()` /
  `::introspection_mtls_client_arc()`** — `#[doc(hidden)]`
  accessors used by the watcher to fail-closed at construction
  time and by tests to verify rotation. Operator code should
  not need to call these directly.

### Changed

- `docs/recipes/mtls_rotation.md` — promotes the new
  filesystem-watcher path to "recommended" with an end-to-end
  Python snippet; the Phase 33 hand-rolled polling loop is kept
  as a fallback for slim wheels / restricted containers.

## [0.35.0] - 2026-05-02

Phase 34 — Public-release prep, tech-debt + docs sweep. Closes the
long-deferred [`plans/PLAN_CLEANUP.md`](plans/PLAN_CLEANUP.md) backlog (placeholder
substitution, OSS hygiene files) and brings the mkdocs site current
with every feature shipped through Phase 33. Original Phase 34
candidates (trait-based `MtlsIdentityProvider`, automatic
refresh-on-handshake-failure, filesystem-watcher integration) are
postponed to Phase 35+ — see
[PLAN.md → Phase 35 candidates](PLAN.md). No code changes other than
the rustdoc URL fix in `tako-core/src/lib.rs` and the version bump.
Plan: [plans/PLAN_PHASE34.md](plans/PLAN_PHASE34.md).

### Changed

- **Phase 34.A — `TODO(<org>)` substitution.** Substituted literal
  `TODO(<org>)` → `nyankobu010` across 11 non-self-referential files
  ([Cargo.toml](Cargo.toml#L26), [pyproject.toml](pyproject.toml),
  [mkdocs.yml](mkdocs.yml), [README.md](README.md),
  [CONTRIBUTING.md](CONTRIBUTING.md), [CHANGELOG.md](CHANGELOG.md)
  compare-link footer, [crates/tako-core/src/lib.rs](crates/tako-core/src/lib.rs)
  rustdoc, [docs/concepts/policy.md](docs/concepts/policy.md)).
  Self-referential historical sites at
  [plans/PLAN_PHASE1.md:55](plans/PLAN_PHASE1.md#L55) and
  [plans/PLAN_PHASE21.md:239](plans/PLAN_PHASE21.md#L239) intentionally retained as
  the source of the placeholder strategy.
- **Phase 34.B — `TODO(community)` substitution.**
  [README.md](README.md) community section now points at
  [GitHub Discussions](https://github.com/nyankobu010/tako-ai-core/discussions)
  and [`SECURITY.md`](SECURITY.md). [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)
  routes to the maintainer's GitHub noreply address + Private
  Vulnerability Reporting. [`SECURITY.md`](SECURITY.md) drops the
  email line and routes to PVR.
- **Phase 34.D — Documentation refresh.** Brought
  [`docs/index.md`](docs/index.md) (was pinned to v0.3.0 / Phase 2.5),
  [`docs/architecture.md`](docs/architecture.md) (was "describes Phase
  1"), [`docs/quickstart.md`](docs/quickstart.md), and concept pages
  (`providers`, `orchestrators`, `budgets`, `mcp`, `secrets`) to
  v0.35.0 / Phase 34 parity. Removed forward-tense markers
  ("Phase 4 will add…") for capabilities that have shipped.
- **Phase 34.F — Mkdocs nav update.** [`mkdocs.yml`](mkdocs.yml) nav
  now lists all the new concept and recipe pages.
- **Phase 34.H — CHANGELOG anchors.** Compare-link footer extended
  from v0.14.0 (stale) through v0.34.0, and the org swapped from
  `TODO(<org>)` to `nyankobu010`.
- **Phase 34.I — Version bump.** Workspace + Python facade both at
  v0.35.0.

### Added

- **Phase 34.C — OSS hygiene files.**
  [`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md),
  [`.github/CODEOWNERS`](.github/CODEOWNERS),
  [`.github/workflows/dco.yml`](.github/workflows/dco.yml) (enforces the
  DCO sign-off mandate at [CONTRIBUTING.md:41](CONTRIBUTING.md#L41) —
  was previously honour-system only),
  [`SUPPORT.md`](SUPPORT.md), [`CITATION.cff`](CITATION.cff).
- **Phase 34.E — New documentation pages.** Five new concept pages
  ([vision](docs/concepts/vision.md),
  [url_prefetch](docs/concepts/url_prefetch.md),
  [streaming](docs/concepts/streaming.md),
  [compat](docs/concepts/compat.md),
  [sigstore](docs/concepts/sigstore.md))
  and eight new recipes ([mistral](docs/recipes/mistral.md),
  [ollama](docs/recipes/ollama.md),
  [vision](docs/recipes/vision.md),
  [url_prefetch](docs/recipes/url_prefetch.md),
  [oidc_introspection](docs/recipes/oidc_introspection.md),
  [chained_auth](docs/recipes/chained_auth.md),
  [mtls_rotation](docs/recipes/mtls_rotation.md),
  [sigstore_keyless](docs/recipes/sigstore_keyless.md)).
- **Phase 34.G — Sanity script.**
  [`scripts/check_public_release.sh`](scripts/check_public_release.sh)
  — eight-check public-release gate (placeholder sweep, secrets scan,
  version consistency, mkdocs strict build, cargo fmt/clippy/test,
  ruff/pytest).

## [0.34.0] - 2026-05-02

Phase 33 — OIDC mTLS cert/key rotation. Closes the
Phase-24/25-deferred operator-UX gap where the mTLS
introspection client was built once at builder time and
required a process restart to refresh. Phase 33 adds an
explicit-reload primitive: operators call
`OidcAuthResolver::reload_mtls_identity(cert_pem, key_pem)`
from their own scheduler (cert-manager webhook, Vault PKI
rotation, filesystem watcher, periodic poll) and the next
request uses the new identity. The swap is atomic from the
request-handler's perspective — concurrent introspection POSTs
either see the old Client or the new one, never a torn state.

Two sub-items, mostly additive (one internal field type
widening on a 6-week-old struct). Plan:
[plans/PLAN_PHASE33.md](plans/PLAN_PHASE33.md).

### Added

- **Phase 33.A — Rust core
  ([crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs)).**

  New public `MtlsClient` newtype wraps an
  `std::sync::RwLock<Arc<reqwest::Client>>`. `MtlsClient::new`
  constructs from a freshly-built reqwest Client;
  `MtlsClient::current()` returns an `Arc<reqwest::Client>`
  snapshot (lock acquisition is brief — read lock + Arc
  clone; poison-recoverable);
  `MtlsClient::swap(client)` atomically replaces the inner
  Client. Concurrent readers either see the old Client or the
  new one, never a torn state.

  Phase 24/25 builders
  (`with_introspection_mtls{,_combined}` and
  `with_introspection_self_signed_mtls{,_combined}`) now wrap
  the freshly-built `reqwest::Client` in `MtlsClient::new(...)`
  before storing on `IntrospectionConfig.mtls_client`.

  `introspect()` request-time read path snapshots via
  `cfg.mtls_client.as_ref().map(|m| m.current())` before
  building the request. The snapshot lives for the duration
  of the request; concurrent reloads via
  `OidcAuthResolver::reload_mtls_identity` affect only the
  NEXT request, never an in-flight one.

  New `OidcAuthResolver::reload_mtls_identity(cert_pem,
  key_pem) -> Result<(), TakoError>` and
  `reload_mtls_identity_combined(combined_pem)` methods. Both
  take `&self` (not `&mut self`) so operators can call through
  a shared `Arc<OidcAuthResolver>` — interior mutability lives
  on the `MtlsClient::inner` RwLock. Reload errors when no
  prior `with_introspection_mtls` /
  `with_introspection_self_signed_mtls` call (operator
  notices early; not silent no-op). Reload PEM parse /
  `reqwest::Client` build failures surface as
  `TakoError::Invalid` AND leave the previously installed
  Client unchanged.

  Seven new unit tests:
  - `mtls_client_current_returns_arc_clone` — Two `current()`
    calls before any swap return Arc-equal snapshots.
  - `mtls_client_swap_replaces_inner` — After `swap()`,
    `current()` returns a NEW Arc.
  - `reload_mtls_identity_swaps_under_arc_resolver` — Reload
    through `Arc<OidcAuthResolver>` works via `&self`.
  - `reload_mtls_identity_errs_when_no_mtls_configured` —
    Resolver without prior mTLS call returns `TakoError::Invalid`
    with operator guidance pointing at the right builder.
  - `reload_mtls_identity_errs_on_invalid_pem_and_preserves_old`
    — Garbage PEM returns Err AND the previously installed
    Client is still served by `current()` (no
    partial-rollback).
  - `reload_mtls_identity_combined_works_for_combined_pem` —
    `cat cert.pem key.pem` form roundtrips.
  - `reload_mtls_identity_works_for_self_signed_too` — Reload
    works identically for Phase 25 self-signed mTLS configs.

- **Phase 33.B — Python facade
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)
  +
  [python/tako/compat.py](python/tako/compat.py)).**

  New `PyOidcAuth.reload_mtls_identity(cert_pem, key_pem)` and
  `reload_mtls_identity_combined(combined_pem)` methods. Both
  take `&self` (not the immutable-builder pattern) and mutate
  state in place via internal mutability. Both raise
  `ValueError` (mapped from `TakoError::Invalid` via
  `map_err`) when no prior `with_introspection_mtls` /
  `with_introspection_self_signed_mtls` call AND when the new
  PEM fails to parse / the reqwest Client fails to build.

  [`python/tako/compat.py`](python/tako/compat.py) module
  docstring extended with a Phase 33.B paragraph describing
  the new methods, the cert-rotation use case, and the
  atomic-swap semantic.

  Three new tests in
  [`tests/python/test_phase33_oidc_mtls_reload.py`](tests/python/test_phase33_oidc_mtls_reload.py)
  pin attribute presence on both methods and confirm the
  Phase 33.B paragraph appears in the `serve_openai`
  docstring (documentation-discoverability follows the Phase
  24.B cadence).

### Changed

- Workspace + Python crate version bumped to v0.34.0.
- `IntrospectionConfig.mtls_client` field type widens from
  `Option<Arc<reqwest::Client>>` to
  `Option<Arc<MtlsClient>>`. The struct is barely 6 weeks old
  (Phase 24); external callers who pass `None` are unaffected;
  callers who construct with `Some(Arc::new(client))` need to
  wrap in `MtlsClient::new(...)`. The Phase 24 + 25 builders
  do the wrapping; only callers who construct
  `IntrospectionConfig` directly with `Some(...)` are
  affected.

### Carried forward to Phase 34+

- **Trait-based `MtlsIdentityProvider`** — async trait that
  yields fresh cert+key bytes on demand; tako would call it
  proactively at e.g. 90% of cert validity. Needs cert-parsing
  on the tako side (`x509-parser` dep or hand-rolled DER
  walk).
- **Automatic refresh-on-handshake-failure** — catch TLS
  handshake errors at request time and trigger reload. Needs
  retry logic + cycle-detection.
- **Filesystem watcher integration** — auto-reload when the
  cert+key files on disk change. `notify` crate dep.
- **OIDC mTLS end-to-end integration test** (Phase 24/25
  carry-forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).
- **`TakoError::Provider` short-circuit on
  `ChainedAuthResolver`** (Phase 27 carry-forward).

## [0.33.0] - 2026-05-02

Phase 32 — URL pre-fetch CIDR allowlist. Closes the
Phase-31-deferred operator-UX gap where allowlists could only
match host strings (exact + wildcard suffix). Operators with
private subnets hosting many dynamic hosts — or raw IP literals
with no DNS at all — now get a CIDR-based bypass:
`with_url_prefetch_allow_cidr("10.0.5.0/24")` permits any IP in
that subnet whether reached via hostname resolution or as an IP
literal in the URL.

After Phase 32 the operator allowlist surface covers three
semantic forms:
  - Exact string  ("registry.corp")     — URL host string
  - Wildcard      ("*.internal.corp")   — URL host suffix
  - CIDR subnet   ("10.0.5.0/24")       — Resolved IP (any)

Three sub-items, all strictly additive — public APIs unchanged
shape. Plan: [plans/PLAN_PHASE32.md](plans/PLAN_PHASE32.md).

### Added

- **Phase 32.A — Bedrock URL pre-fetch CIDR allowlist
  ([crates/tako-providers/bedrock/src/url_prefetch.rs](crates/tako-providers/bedrock/src/url_prefetch.rs)
  +
  [crates/tako-providers/bedrock/src/client.rs](crates/tako-providers/bedrock/src/client.rs)).**

  New `ipnet = "2"` workspace dep (small, well-maintained,
  zero transitive deps). `AllowList` gains `cidrs: Vec<IpNet>`
  field; constructor renamed `from_strings(hosts) -> Self`
  → `from_strings_and_cidrs(hosts, cidrs) -> Result<Self,
  TakoError>` with parse-time CIDR validation. CIDR parse
  failures surface from the builder as `TakoError::Invalid`
  so operators notice early — consistent with Phase 24/25 mTLS
  PEM parse-time failure cadence.

  New `AllowList::contains_ip(&IpAddr) -> bool` checks if an
  IP falls inside any allowlisted CIDR. The runtime check is a
  short linear scan over CIDRs (typical operator allowlists are
  small).

  `BlocklistResolver::resolve` and `fetch_one`'s inline IP-
  literal check both gain CIDR honouring: bypass triggers when
  EITHER the host string is allowlisted (Phase 30/31) OR the
  IP is in an allowlisted CIDR (Phase 32). Per-IP check in the
  resolver — a host that resolves only to allowlisted-CIDR IPs
  is allowed even if the hostname itself isn't allowlisted.

  New `UrlPrefetchOpts.allow_cidrs: Vec<String>` builder field.
  New `BedrockBuilder::with_url_prefetch_allow_cidr(cidr)`
  chainable builder. Does NOT auto-enable
  `with_url_prefetch()`.

  Eight new unit tests covering: IPv4 CIDR match/no-match
  (`10.0.5.0/24`), IPv6 CIDR match/no-match (`2001:db8::/32`),
  single-host `/32`, invalid-CIDR parse error, three-mode
  coexistence (exact + wildcard + CIDR in one allowlist),
  end-to-end wiremock with `127.0.0.0/8` allowlist permitting
  the loopback binding.

- **Phase 32.B — Ollama URL pre-fetch CIDR allowlist
  ([crates/tako-providers/ollama/src/url_prefetch.rs](crates/tako-providers/ollama/src/url_prefetch.rs)
  +
  [crates/tako-providers/ollama/src/client.rs](crates/tako-providers/ollama/src/client.rs)).**
  Per-crate copy of all 32.A surfaces. Per ARCHITECTURE.md
  hard rule, the `AllowList` struct is duplicated rather than
  shared. Same `OllamaBuilder::with_url_prefetch_allow_cidr(cidr)`
  builder. Same test surface (8 new unit tests).

- **Phase 32.C — Python facade
  ([crates/tako-py/src/py_bedrock.rs](crates/tako-py/src/py_bedrock.rs)
  +
  [crates/tako-py/src/py_ollama.rs](crates/tako-py/src/py_ollama.rs)
  +
  [python/tako/providers.py](python/tako/providers.py)).**

  Both `PyBedrock::new` and `PyOllama::new` gain a
  `url_prefetch_allow_cidrs: Option<Vec<String>>` kwarg
  (positioned between `url_prefetch_allow_hosts` and
  `url_prefetch_timeout_secs`). When `Some(cidrs)`, the PyO3
  ctor calls `with_url_prefetch_allow_cidr(cidr)` for each
  entry on the underlying builder. `None` (default) means
  empty CIDR list. CIDR parse failures surface from the
  constructor (TakoError::Invalid → Python exception via
  `map_err`).

  [`python/tako/providers.py`](python/tako/providers.py): both
  `Bedrock` and `Ollama` `__init__` gain the new kwarg. Both
  class docstrings rewritten to describe the three allowlist
  forms (exact host, wildcard host, CIDR subnet) side-by-side
  with examples. The Ollama docstring example shows all three
  forms together.

  [`python/tako/_native.pyi`](python/tako/_native.pyi) extended
  with the new kwarg on both stubs.

  Seven new tests in
  [`tests/python/test_phase32_allow_cidrs.py`](tests/python/test_phase32_allow_cidrs.py)
  pin: kwarg presence on both providers; default `None`;
  docstring documents the kwarg with CIDR examples; Bedrock
  docstring mentions all three allowlist forms together
  (`url_prefetch_allow_hosts` + `url_prefetch_allow_cidrs` +
  `*.` wildcard syntax marker).

### Changed

- Workspace + Python crate version bumped to v0.33.0.
- `UrlPrefetchOpts` gains `allow_cidrs: Vec<String>` field
  (default empty; existing callers unaffected).
- `AllowList::from_strings(...)` renamed to
  `from_strings_and_cidrs(hosts, cidrs)` and the return type
  widens to `Result<Self, TakoError>`. Internal pub(crate)
  signature only — no public-API impact.

### Carried forward to Phase 33+

- **Wildcard at non-leftmost positions** — patterns like
  `registry.*.corp`. No operator ask yet.
- **Strict-allowlist mode** — currently all allowlists are
  per-rule BYPASSes of the blocklist. A strict mode would
  REQUIRE every URL host to match an allowlist entry.
- **OIDC mTLS end-to-end integration test** (Phase 24/25
  carry-forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).
- **`TakoError::Provider` short-circuit on
  `ChainedAuthResolver`** (Phase 27 carry-forward).

## [0.32.0] - 2026-05-01

Phase 31 — URL pre-fetch wildcard host patterns. Closes the
Phase-30-deferred operator-UX gap where exact-string allowlist
entries had to enumerate every subdomain. A single
`*.internal.corp` entry now covers all current AND future
subdomains under that suffix.

Wildcard semantic: `*.X` matches any hostname ending in `.X`
(literal `ends_with` check), INCLUDING multi-level subdomains
(`staging.images.internal.corp` matches `*.internal.corp`).
Does NOT match the bare apex (`internal.corp`) — operators add
the apex as a separate exact entry if needed. Multi-level
matching is the operator-intent default; RFC 6125's strict
one-level semantics is for TLS cert SANs, not operator-
controlled allowlists.

Three sub-items, all strictly additive — public APIs unchanged
shape. Plan: [plans/PLAN_PHASE31.md](plans/PLAN_PHASE31.md).

### Added

- **Phase 31.A — Bedrock URL pre-fetch wildcard host patterns
  ([crates/tako-providers/bedrock/src/url_prefetch.rs](crates/tako-providers/bedrock/src/url_prefetch.rs)
  +
  [crates/tako-providers/bedrock/src/client.rs](crates/tako-providers/bedrock/src/client.rs)).**

  New `AllowList` struct splits exact-match hostnames from
  wildcard suffix patterns at config time:

  ```rust
  pub(crate) struct AllowList {
      exact: HashSet<String>,
      suffixes: Vec<String>,  // each entry stored as `.X` for ends_with
  }

  impl AllowList {
      pub(crate) fn from_strings(entries: Vec<String>) -> Self;
      pub(crate) fn contains(&self, host: &str) -> bool;
  }
  ```

  Entries starting with `*.` are recognised at `from_strings`
  time and stored in `suffixes` (with the leading `*` stripped
  and the result prefixed with `.` for `ends_with`). Phase 30
  entries (no `*.` prefix) continue to flow into the `exact`
  HashSet — semantics preserved byte-for-byte.

  Runtime check (`AllowList::contains`) is a single
  `HashSet::contains` plus a short linear scan over dotted
  suffixes — no per-call `format!` allocation.

  `Arc<HashSet<String>>` becomes `Arc<AllowList>` on
  `UrlPrefetchConfig` and `BlocklistResolver`. The Phase 30
  builder method `with_url_prefetch_allow_host(host)` is
  unchanged — entries are parsed at `into_config` time. Doc
  comment updated to document both match modes.

  Eight new unit tests covering: exact match (Phase 30
  regression), single-level subdomain match, multi-level
  subdomain match, bare-domain non-match, other-domain
  non-match, attacker-domain non-match (`attacker-internal.corp`
  vs `*.internal.corp`), and exact + wildcard coexistence.

- **Phase 31.B — Ollama URL pre-fetch wildcard host patterns
  ([crates/tako-providers/ollama/src/url_prefetch.rs](crates/tako-providers/ollama/src/url_prefetch.rs)
  +
  [crates/tako-providers/ollama/src/client.rs](crates/tako-providers/ollama/src/client.rs)).**
  Per-crate copy of all 31.A surfaces. Per ARCHITECTURE.md
  hard rule (provider crates depend only on `tako-core` +
  their vendor SDK + reqwest; never on each other), the
  `AllowList` struct is duplicated rather than shared. Same
  test surface (8 new unit tests).

- **Phase 31.C — Python facade docstrings + tests
  ([python/tako/providers.py](python/tako/providers.py)
  +
  [tests/python/test_phase31_wildcard_hosts.py](tests/python/test_phase31_wildcard_hosts.py)).**

  No PyO3 code change — the
  `url_prefetch_allow_hosts: list[str] | None` kwarg shape is
  unchanged; the new wildcard semantic lands entirely on the
  Rust side. The Python facade ships:

  - Both `Bedrock` and `Ollama` class docstrings updated to
    document the two match modes (exact-string + wildcard
    suffix), the multi-level matching semantic, and the
    bare-apex caveat.
  - The `Ollama` docstring example was extended to show both
    modes side-by-side.
  - Six new tests in
    [`tests/python/test_phase31_wildcard_hosts.py`](tests/python/test_phase31_wildcard_hosts.py)
    pin: kwarg type unchanged (Phase 30 regression); both
    docstrings document `*.X` patterns with multi-level
    matching; both docstrings include the bare-apex caveat.

### Changed

- Workspace + Python crate version bumped to v0.32.0.
- `UrlPrefetchConfig::new`'s fifth parameter widens from
  `Arc<HashSet<String>>` to `Arc<AllowList>`. Internal
  pub(crate) signature only — no public-API impact.

### Carried forward to Phase 32+

- **CIDR allowlist** — `with_url_prefetch_allow_cidr("10.0.5.0/24")`.
  Operators may want to permit a whole subnet without
  enumerating each host. Needs a CIDR parser dep
  (`ipnet` or hand-rolled).
- **Wildcard at non-leftmost positions** — patterns like
  `registry.*.corp`. Phase 31 ships only the leftmost-`*.`
  convention. Probably never worth shipping unless a real
  operator asks.
- **OIDC mTLS end-to-end integration test** (Phase 24/25
  carry-forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).
- **`TakoError::Provider` short-circuit on
  `ChainedAuthResolver`** (Phase 27 carry-forward).
- **Per-child `ChainedAuthResolver` policy override**
  (Phase 27 carry-forward).

## [0.31.0] - 2026-05-01

Phase 30 — URL pre-fetch per-host allowlist. Closes the
Phase-29-deferred operator-UX gap where the binary
`with_url_prefetch_allow_private_ips()` flag is a sledgehammer:
operators with an internal artifact registry on a private RFC
1918 address would have to disable the WHOLE blocklist (incl.
the canary 169.254.169.254 cloud-metadata endpoint) just to
permit one trusted host.

Phase 30 adds a per-host BYPASS that lets operators allowlist
specific hostnames while keeping the rest of the blocklist
active. Allowlisted hosts skip ONLY the private-IP blocklist;
scheme / timeout / size cap / MIME validation all still apply
(defence-in-depth). Plan: [plans/PLAN_PHASE30.md](plans/PLAN_PHASE30.md).

### Added

- **Phase 30.A — Bedrock URL pre-fetch per-host allowlist
  ([crates/tako-providers/bedrock/src/url_prefetch.rs](crates/tako-providers/bedrock/src/url_prefetch.rs)
  +
  [crates/tako-providers/bedrock/src/client.rs](crates/tako-providers/bedrock/src/client.rs)).**

  New `UrlPrefetchConfig.allow_hosts: Arc<HashSet<String>>`
  field shared between the `BlocklistResolver` and the inline
  IP-literal check via `Arc` (cheap clone). The
  `BlocklistResolver` carries a clone and skips the per-IP
  blocklist when the requested hostname is in the set. The
  inline IP-literal check (Phase 29.A) gains the same bypass:
  matched against the raw `host_str` so
  `with_url_prefetch_allow_host("10.0.5.4")` matches a URL
  whose host is exactly `10.0.5.4`.

  `UrlPrefetchOpts.allow_hosts: Vec<String>` builder field;
  `into_config` collects to `Arc<HashSet<String>>` so duplicate
  builder calls dedupe naturally.

  New `BedrockBuilder::with_url_prefetch_allow_host(host: impl
  Into<String>)` builder method — chainable; can be called
  multiple times. Does NOT auto-enable `with_url_prefetch()`
  (master switch must already be on).

  Five new unit tests pinning: default empty allowlist,
  into_config round-trip + dedupe, IP-literal allowlist bypass
  (wiremock on 127.0.0.1 with allowlist={"127.0.0.1"}), and the
  exact-match semantics (allowlist={"some-other-host"} does NOT
  bypass `127.0.0.1`).

- **Phase 30.B — Ollama URL pre-fetch per-host allowlist
  ([crates/tako-providers/ollama/src/url_prefetch.rs](crates/tako-providers/ollama/src/url_prefetch.rs)
  +
  [crates/tako-providers/ollama/src/client.rs](crates/tako-providers/ollama/src/client.rs)).**
  Per-crate copy of all 30.A surfaces (per ARCHITECTURE.md
  hard rule — provider crates depend only on `tako-core` +
  their vendor SDK + reqwest; never on each other). Same
  `OllamaBuilder::with_url_prefetch_allow_host(host)` builder.
  Same test surface (5 new unit tests).

- **Phase 30.C — Python facade
  ([crates/tako-py/src/py_bedrock.rs](crates/tako-py/src/py_bedrock.rs)
  +
  [crates/tako-py/src/py_ollama.rs](crates/tako-py/src/py_ollama.rs)
  +
  [python/tako/providers.py](python/tako/providers.py)).**
  Both `tako.providers.Bedrock` and `tako.providers.Ollama`
  gain a new `url_prefetch_allow_hosts: list[str] | None`
  kwarg, positioned between `url_prefetch_allow_private_ips`
  and `url_prefetch_timeout_secs`. When `Some(hosts)`, the
  PyO3 ctor calls
  `with_url_prefetch_allow_host(host)` for each entry on the
  underlying builder. `None` (default) means empty allowlist
  — existing callers unaffected.

  [`python/tako/_native.pyi`](python/tako/_native.pyi) updated
  with the new kwarg on both stubs. Both class docstrings
  document the new kwarg + bypass semantics.

  Six new tests in
  [`tests/python/test_phase30_allow_hosts.py`](tests/python/test_phase30_allow_hosts.py)
  pin: kwarg presence on both providers; default `None`;
  docstring documents the kwarg + bypass semantic. Behaviour
  pinned in the Rust unit tests.

### Changed

- Workspace + Python crate version bumped to v0.31.0.
- `UrlPrefetchConfig::new` widens to take a fifth
  `allow_hosts: Arc<HashSet<String>>` parameter. Internal
  pub(crate) signature only — no public-API impact.

### Carried forward to Phase 31+

- **Wildcard / suffix host patterns** — Phase 30 ships
  exact-string match only. Operators may want
  `*.internal.corp.local` to permit all subdomains. Needs a
  pattern matcher.
- **CIDR allowlist** — `with_url_prefetch_allow_cidr("10.0.5.0/24")`
  to permit a whole subnet without enumerating each host.
  Needs a CIDR parser dep.
- **OIDC mTLS end-to-end integration test** (Phase 24/25
  carry-forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).
- **`TakoError::Provider` short-circuit on
  `ChainedAuthResolver`** (Phase 27 carry-forward).
- **Per-child `ChainedAuthResolver` policy override**
  (Phase 27 carry-forward).

## [0.30.0] - 2026-05-01

Phase 29 — URL pre-fetch SSRF hardening + Ollama Python facade.
Closes the Phase 28-deferred CIDR-block + DNS-rebinding
mitigation gap, and the Phase 28.C asymmetry where
`url_prefetch` was threaded through `tako.providers.Bedrock`
but not Ollama (which had no Python binding in tako-py).

The Phase 28 SSRF mitigations were `https`-only / timeout /
size cap / MIME validation; operators were left to enforce
network egress at deployment level. Phase 29 adds
defence-in-depth at two layers: (a) a custom DNS resolver
that rejects private/loopback/link-local/multicast/IPv6-
unique-local IPs at resolve time AND validates EVERY returned
`SocketAddr` (closing the DNS-rebinding window); (b) an inline
IP-literal check for URLs whose host is already an IP (where
reqwest skips the resolver). Default-on; opt out via the new
`with_url_prefetch_allow_private_ips()` builder for deployments
that already filter network egress.

After Phase 29 the tako-side URL pre-fetch surface ships with a
complete SSRF-mitigation stack, and both URL-prefetching
providers (Bedrock + Ollama) have full Python parity.
Plan: [plans/PLAN_PHASE29.md](plans/PLAN_PHASE29.md).

### Added

- **Phase 29.A — Bedrock URL pre-fetch private-IP blocklist +
  DNS-rebind mitigation
  ([crates/tako-providers/bedrock/src/url_prefetch.rs](crates/tako-providers/bedrock/src/url_prefetch.rs)).**
  New public-to-crate `is_blocked_ip(&IpAddr) -> bool` helper
  rejects: IPv4 loopback (`127/8`), RFC 1918 (`10/8`,
  `172.16/12`, `192.168/16`), link-local (`169.254/16`),
  unspecified (`0.0.0.0`), broadcast (`255.255.255.255`),
  multicast (`224/4`), and reserved (`240/4`); IPv6 loopback
  (`::1`), unspecified (`::`), multicast (`ff00::/8`),
  unique-local (`fc00::/7`), unicast-link-local (`fe80::/10`),
  and IPv4-mapped variants (`::ffff:x.x.x.x`) recursively
  checked via `Ipv6Addr::to_ipv4_mapped` (stable on workspace
  MSRV 1.85). Pure stdlib; no new deps.

  New `BlocklistResolver` impl of `reqwest::dns::Resolve` wraps
  `tokio::net::lookup_host` and validates EVERY returned
  `SocketAddr` against `is_blocked_ip`. Validating all
  addresses (not just first) is the DNS-rebinding mitigation —
  a malicious resolver returning two A records (one public,
  one private) can't slip the private IP through alongside a
  public one, and there's no second resolution between
  validation and connection.

  New inline IP-literal check in `fetch_one` after URL parse:
  reqwest skips the DNS resolver for IP-literal URLs (e.g.
  `http://127.0.0.1/...`), so the blocklist must be enforced
  here too. Parses `host_str` as `IpAddr` (stripping IPv6
  brackets); on parse failure it's a domain name and the
  resolver path takes over.

  `UrlPrefetchOpts.block_private_ips: bool` field defaults to
  `true` (Phase 29 default-deny stance for SSRF). Plumbed
  through `UrlPrefetchConfig::new()` to conditionally install
  the resolver. New
  `BedrockBuilder::with_url_prefetch_allow_private_ips()`
  builder method opts out for deployments where the network
  layer already filters egress. Does NOT auto-enable the
  master `with_url_prefetch()` switch.

  Thirteen new unit tests covering all blocked + allowed IP
  categories (including the `169.254.169.254` cloud-metadata
  canary and `::ffff:127.0.0.1` IPv4-mapped variant); two
  wiremock integration tests pinning end-to-end loopback
  rejection and the operator opt-out.

- **Phase 29.B — Ollama URL pre-fetch private-IP blocklist +
  DNS-rebind mitigation
  ([crates/tako-providers/ollama/src/url_prefetch.rs](crates/tako-providers/ollama/src/url_prefetch.rs)).**
  Per-crate copy of all 29.A surfaces (per ARCHITECTURE.md
  hard rule — provider crates depend only on `tako-core` +
  their vendor SDK + reqwest; never on each other). Phase 28.B
  established the duplication; Phase 29.B extends each copy.
  Same `is_blocked_ip` / `BlocklistResolver` /
  `UrlPrefetchOpts.block_private_ips` /
  `OllamaBuilder::with_url_prefetch_allow_private_ips()`
  surface. Same test surface as 29.A.

- **Phase 29.C — `tako.providers.Ollama` Python facade +
  `url_prefetch_allow_private_ips` kwarg on Bedrock
  ([crates/tako-py/src/py_ollama.rs](crates/tako-py/src/py_ollama.rs)
  +
  [crates/tako-py/src/py_bedrock.rs](crates/tako-py/src/py_bedrock.rs)
  +
  [python/tako/providers.py](python/tako/providers.py)).**

  New `PyOllama` pyclass mirrors the Phase 28.C `PyBedrock`
  cadence. Constructor signature:
  ```python
  Ollama(
      model: str,
      *,
      base_url: str | None = None,
      timeout_secs: int | None = None,
      url_prefetch: bool = False,
      url_prefetch_allow_http: bool = False,
      url_prefetch_allow_private_ips: bool = False,
      url_prefetch_timeout_secs: int | None = None,
      url_prefetch_max_bytes: int | None = None,
  )
  ```
  `OllamaBuilder::build()` is sync (no async credential chain),
  so the constructor calls `b.build()?` directly without
  `py.detach + rt.block_on` (Bedrock's async path).

  `PyBedrock::new` gains the new
  `url_prefetch_allow_private_ips: bool = False` kwarg between
  `url_prefetch_allow_http` and `url_prefetch_timeout_secs`,
  plumbing through to
  `BedrockBuilder::with_url_prefetch_allow_private_ips()`.

  Wiring:
  [`crates/tako-py/Cargo.toml`](crates/tako-py/Cargo.toml) adds
  `tako-providers-ollama` workspace dep;
  [`crates/tako-py/src/lib.rs`](crates/tako-py/src/lib.rs)
  registers `mod py_ollama` and adds `PyOllama` to the
  `_native` module; [`python/tako/providers.py`](python/tako/providers.py)
  adds new `class Ollama(_ProviderBase)` and the new kwarg on
  `class Bedrock`; [`python/tako/_native.pyi`](python/tako/_native.pyi)
  adds new `Ollama` stub and the extended `Bedrock` stub.

  Six new tests in
  [`tests/python/test_phase29_ssrf_hardening.py`](tests/python/test_phase29_ssrf_hardening.py)
  pin the kwarg presence + default + docstring on both
  providers; seven new tests in
  [`tests/python/test_phase29_ollama_facade.py`](tests/python/test_phase29_ollama_facade.py)
  pin the new `Ollama` class exists, has the expected
  signature, has sensible defaults, and inherits from
  `_ProviderBase`. Both files validate the Python-facing
  *signature* rather than constructing live providers
  (Bedrock needs AWS credentials; Ollama needs a daemon).
  Behaviour pinned in the Rust unit tests.

### Changed

- Workspace + Python crate version bumped to v0.30.0.
- Phase 28 wiremock tests in
  `crates/tako-providers/{bedrock,ollama}/src/url_prefetch.rs`
  now pass `block_private_ips: false` in their
  `UrlPrefetchConfig::new()` calls — they pre-date the new
  Phase 29 default and bind to `127.0.0.1`. Public
  `BedrockBuilder` / `OllamaBuilder` API unchanged byte-for-
  byte for callers who didn't opt in to URL pre-fetch.

### Carried forward to Phase 30+

- **Per-domain allowlist for URL pre-fetch** — operators may
  want to permit specific internal hostnames (e.g., a private
  artifact registry on `10.0.x.x`) while still blocking
  everything else. Phase 29 ships only the binary on/off
  allow-private-IPs flag; an allowlist would need a chainable
  `with_url_prefetch_allow_host(host)` builder + matching
  Python kwarg.
- **OIDC mTLS end-to-end integration test** (Phase 24/25
  carry-forward).
- **OIDC mTLS cert / key rotation** for long-running
  deployments rotating client certs (Phase 24/25 carry-
  forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).
- **`TakoError::Provider` short-circuit on
  `ChainedAuthResolver`** (Phase 27 carry-forward).
- **Per-child `ChainedAuthResolver` policy override**
  (Phase 27 carry-forward).

## [0.29.0] - 2026-05-01

Phase 28 — closes the URL-source-image gap on Bedrock + Ollama.
Phase 22 + Phase 23 shipped URL-source images on the four
providers whose API servers fetch URLs themselves (Anthropic +
OpenAI + Mistral + Vertex). Bedrock's `ImageSource` has no URL
variant, and Ollama's `images: Vec<String>` field requires bare
base64 — for those two, tako must fetch the URL itself. Phase
28 ships that with security-conscious defaults: opt-in
default-off, `https`-only by default, configurable timeout / size
cap (with `Content-Length` pre-flight + post-fetch byte-count
defence-in-depth), MIME validation against the four supported
types (`image/{jpeg,png,gif,webp}`). CIDR-block egress filtering
and DNS-rebinding mitigation are explicitly NOT in scope —
operators must enforce network egress at the deployment level
(VPC egress rules, Pod-level egress NetworkPolicies, etc).
After Phase 28 every shipped provider adapter (Anthropic +
OpenAI + Mistral + Vertex + Bedrock + Ollama — six of six)
handles outbound `ContentPart::ImageUrl`. Plan: [plans/PLAN_PHASE28.md](plans/PLAN_PHASE28.md).

### Added

- **Phase 28.A — Bedrock URL pre-fetch
  ([crates/tako-providers/bedrock/src/url_prefetch.rs](crates/tako-providers/bedrock/src/url_prefetch.rs)).**
  New `UrlPrefetchConfig` struct with
  `allow_http: bool` / `max_bytes: usize` / `http: reqwest::Client`
  fields. `UrlPrefetchConfig::rewrite(&mut ChatRequest)` walks
  every message's content array; for each
  `ContentPart::ImageUrl { url, mime }`, fetches the URL,
  validates the response MIME against the four supported types,
  base64-encodes the body via `aws_smithy_types::base64::encode`
  (already in the dep tree), and replaces the content part with
  `ContentPart::Image { mime, data_b64 }` in place. Other content
  parts pass through unchanged. URL parsing uses
  `reqwest::Url::parse` for the scheme check
  (`http`/`https` only; `https`-only by default).
  `BedrockBuilder` gains four new methods:
  `with_url_prefetch(self, enabled: bool)`,
  `with_url_prefetch_allow_http(self, allow: bool)`,
  `with_url_prefetch_timeout(self, secs: u64)`,
  `with_url_prefetch_max_bytes(self, bytes: usize)`. The
  `Inner` struct gains `url_prefetch: Option<UrlPrefetchConfig>`;
  `chat()` and `stream()` call `prefetch.rewrite(&mut req).await?`
  before the existing convert step. New `reqwest = workspace`
  + `wiremock` dev-dep added to `bedrock/Cargo.toml`. Eight new
  unit tests + four wiremock integration tests covering the
  rewrite path, scheme rejection, MIME validation, size cap
  (both pre-flight `Content-Length` rejection and post-fetch
  byte-count rejection), and timeout enforcement.

- **Phase 28.B — Ollama URL pre-fetch
  ([crates/tako-providers/ollama/src/url_prefetch.rs](crates/tako-providers/ollama/src/url_prefetch.rs)).**
  Mirror of Bedrock's structure with per-crate copies of the
  helpers (per ARCHITECTURE.md hard rule — provider crates
  depend only on `tako-core` + their vendor SDK + `reqwest`;
  never on each other). Uses
  `base64::engine::general_purpose::STANDARD.encode` instead of
  the AWS SDK's helper (Bedrock has `aws_smithy_types` already
  in the dep tree; Ollama doesn't). New
  `base64 = workspace` dep added to `ollama/Cargo.toml`.
  `OllamaBuilder::with_url_prefetch_*` methods + `Inner.url_prefetch`
  + `chat()` / `stream()` rewrite call wired identically to
  Bedrock. Three new unit tests.

- **Phase 28.C — Python facade
  ([crates/tako-py/src/py_bedrock.rs](crates/tako-py/src/py_bedrock.rs)
  + [python/tako/providers.py](python/tako/providers.py)).**
  `tako.providers.Bedrock(...)` gains four new keyword arguments
  mirroring the Rust builder: `url_prefetch: bool = False`,
  `url_prefetch_allow_http: bool = False`,
  `url_prefetch_timeout_secs: int | None = None`,
  `url_prefetch_max_bytes: int | None = None`. Defaults
  preserve byte-for-byte the Phase 27 zero-config behaviour.
  Type stub
  ([python/tako/_native.pyi](python/tako/_native.pyi)) updated.
  Class docstring updated with SSRF-mitigation summary +
  operator-level network-egress reminder. Ollama has no Python
  binding (no entry in tako-py) — Python surface is Bedrock-only
  for Phase 28. New
  [tests/python/test_phase28_url_prefetch.py](tests/python/test_phase28_url_prefetch.py)
  validates kwarg presence, default values, and docstring
  documentation; behaviour is asserted on the Rust side (15
  tests across 28.A + 28.B).

### Changed

- Workspace + Python crate version bumped to v0.29.0.

### Carried forward to Phase 29+

- CIDR-block egress filtering — opt-in CIDR allowlist /
  blocklist on the URL-prefetch resolver. Operators currently
  enforce at the network layer (VPC egress, NetworkPolicies);
  in-tako filtering would be defence-in-depth.
- DNS-rebinding mitigation — resolve the host once and pin the
  IP across the connection. Pairs with the CIDR work.
- Vertex File API URI scheme on `fileData` — Phase 23 noted
  Vertex's File API (`projects/.../files/...` URIs) is out of
  scope; needs an upload helper to round-trip a local image
  through the File API endpoint. Not yet asked for.
- `tako.providers.Ollama` Python binding — Phase 28.C ships
  Bedrock-only because Ollama has no entry in tako-py today.
  Adding it would mirror `PyBedrock` byte-for-byte.

## [0.28.0] - 2026-05-01

Phase 27 — closes the Phase-26 carry-forward by extending
`ChainedAuthResolver`'s opt-in fail-fast to four "definitely
infrastructure / operator-set guard" `TakoError` variants:
`Transport`, `RateLimited`, `CircuitOpen`, `BudgetExhausted`.
Plan: [plans/PLAN_PHASE27.md](plans/PLAN_PHASE27.md).

The case-by-case analysis on which variants to short-circuit:

- `Transport(String)` — network failure; falling through to
  another resolver gets the wrong-cause error. **Short-circuit ✓**
- `RateLimited(Duration)` — falling through doesn't reset the
  upstream rate limit. **Short-circuit ✓**
- `CircuitOpen` — internal failsafe; falling through doesn't
  reset. **Short-circuit ✓**
- `BudgetExhausted(String)` — operator-set cap; falling through
  circumvents it. **Short-circuit ✓**
- `Provider { ... }` — vendor error; could be auth-related.
  **Fall through** (deferred pending finer discrimination)
- `Invalid(String)` — auth decision. **Fall through**
- `PolicyDenied(String)` — policy decision. **Fall through**

### Added

- **Phase 27.A — `ChainedAuthResolver::with_short_circuit_on_infrastructure_errors`
  ([crates/tako-compat/src/auth/chained.rs](crates/tako-compat/src/auth/chained.rs)).**

  Internal refactor: the Phase-26
  `short_circuit_on_transport_error: bool` field is upgraded
  to a `ShortCircuitPolicy` enum (`None` / `TransportOnly` /
  `AllInfrastructure`). The enum is private; public-API churn
  is zero. Phase 26 callers
  (`with_short_circuit_on_transport_error()` +
  `short_circuits_on_transport_error()`) work byte-for-byte.

  New surfaces:
  - `ChainedAuthResolver::with_short_circuit_on_infrastructure_errors()`
    builder method (idempotent). Last-write-wins between this
    and the Phase-26 narrower builder — the policy is
    overwritten, not merged.
  - `ChainedAuthResolver::short_circuits_on_infrastructure_errors() -> bool`
    accessor. Returns `true` only when the broader builder was
    the most recent policy setter.
  - The Phase-26 `short_circuits_on_transport_error()` accessor
    now returns `true` for both `TransportOnly` and
    `AllInfrastructure` policies (both short-circuit on
    `Transport`).

  `resolve()` extension uses an explicit `match
  self.short_circuit_policy` switch over the three enum
  variants. The `CountingAuth` test mock is extended further to
  preserve `RateLimited` / `CircuitOpen` / `BudgetExhausted`
  variants (currently only `Transport` and `Invalid`
  round-trip; others collapse into `Invalid`).

  Eight new unit tests:

  - `infrastructure_short_circuit_default_falls_through_on_rate_limited`
    — Phase 21 / 26 regression pin.
  - `infrastructure_short_circuit_returns_immediately_on_rate_limited`
  - `infrastructure_short_circuit_returns_immediately_on_circuit_open`
  - `infrastructure_short_circuit_returns_immediately_on_budget_exhausted`
  - `infrastructure_short_circuit_falls_through_on_invalid_error`
    — auth-decision errors still fall through with broader policy.
  - `transport_only_falls_through_on_rate_limited_when_transport_only_set`
    — regression: the Phase-26 narrower flag does NOT broaden
    scope after the policy enum refactor.
  - `short_circuits_on_infrastructure_errors_accessor_reflects_state`
    — both accessors track the policy correctly across the three
    states.
  - `short_circuit_policy_is_last_write_wins` — calling broader
    after narrower (and vice versa) overwrites the policy.

- **Phase 27.B — Python facade
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)).**
  `ChainedAuth.with_short_circuit_on_infrastructure_errors()` +
  `short_circuits_on_infrastructure_errors() -> bool` accessor
  mirror the Rust API. Returns a NEW `ChainedAuth` (immutable
  builder; idempotent). `tako.compat` module docstring updated
  to mention the new builder + the variant coverage. New
  [`tests/python/test_phase27_chained_infrastructure.py`](tests/python/test_phase27_chained_infrastructure.py)
  covers facade attribute presence, immutable-builder semantics,
  last-write-wins between Phase-26 and Phase-27 builders, the
  regression pin that the narrower flag doesn't flip the
  broader accessor, and idempotence.

### Changed

- `ChainedAuthResolver`'s private state widens from a single
  `bool` to a three-state `ShortCircuitPolicy` enum. Public API
  unchanged byte-for-byte; Phase 21 + 26 callers preserve
  semantics.
- The Phase-26 `short_circuits_on_transport_error()` accessor's
  semantics are now "is this chain configured to short-circuit
  on transport errors?" — which is `true` for both narrower
  and broader policies. The doc comment was updated to clarify;
  the boolean output is unchanged for callers who only set the
  Phase-26 narrower flag.
- Workspace + Python crate version bumped to v0.28.0.

### Carried forward to Phase 28+

- `TakoError::Provider` short-circuit — vendor-error
  short-circuit warrants finer discrimination on the embedded
  error. Deferred pending real-world need.
- Per-child `ChainedAuthResolver` policy override — operators
  may want different short-circuit policies per child. Not yet
  asked for.
- OIDC mTLS end-to-end integration test, OIDC mTLS cert
  rotation, URL-source for Bedrock / Ollama, Vertex File API
  upload, eval-harness real graders, OIDC refresh-token /
  revocation.

## [0.27.0] - 2026-05-01

Phase 26 — closes the Phase-21-deferred operator-UX issue with
the `ChainedAuthResolver` fall-through-on-any-Err default. The
Phase 21 PLAN explicitly noted the deferral: "If patterns emerge
for 'fail fast on transport errors' ... a future phase may add
`with_short_circuit_on_transport_error`." Phase 26 ships that
opt-in flag. Plan: [plans/PLAN_PHASE26.md](plans/PLAN_PHASE26.md).

The pattern this addresses: chain
`OidcAuth().then(StaticTokens)`. When the OIDC issuer is
unreachable, OIDC returns `TakoError::Transport(...)`, which the
Phase-21 chain unconditionally falls through to StaticTokens,
which returns `"unknown bearer token"` because the user's OIDC
token isn't in the static map. The end-user sees a misleading
401 with a wrong-cause diagnostic; the operator gets paged for
a wrong-cause symptom. Phase 26's opt-in flag halts the chain
on transport errors, surfacing the actionable
`"transport error: oidc unreachable"` instead.

### Added

- **Phase 26.A — `ChainedAuthResolver::with_short_circuit_on_transport_error`
  ([crates/tako-compat/src/auth/chained.rs](crates/tako-compat/src/auth/chained.rs)).**

  New surfaces:
  - `ChainedAuthResolver.short_circuit_on_transport_error: bool`
    field. Default `false` preserves Phase 21
    fall-through-on-any-Err semantics byte-for-byte.
  - `ChainedAuthResolver::with_short_circuit_on_transport_error()`
    builder method (idempotent).
  - `ChainedAuthResolver::short_circuits_on_transport_error() ->
    bool` accessor.

  `resolve()` extension: when the flag is set AND a child
  returns `Err(TakoError::Transport(_))`, return immediately
  (don't fall through to the next child). Other error variants
  (`TakoError::Invalid`, `PolicyDenied`, etc.) continue to fall
  through — those represent auth decisions the next resolver
  might overturn. Only `Transport` short-circuits in Phase 26;
  broader infrastructure-error semantics (`RateLimited` /
  `CircuitOpen` / `BudgetExhausted` / `Provider` source-error)
  deferred to Phase 27+.

  The existing `CountingAuth` test mock is extended with a
  `Transport`-preserving arm so tests can construct the
  specific error variant; other arms continue to collapse into
  `Invalid` (which the Phase 21 tests rely on).

  Five new unit tests:
  - `short_circuit_default_falls_through_on_transport_error` —
    Phase 21 regression pin: without the flag, transport errors
    fall through exactly like Invalid does.
  - `short_circuit_enabled_returns_immediately_on_transport_error`
    — first child returns `Transport`; the second is **not**
    called; the transport error propagates verbatim.
  - `short_circuit_enabled_falls_through_on_invalid_error` —
    only `Transport` short-circuits.
  - `short_circuit_enabled_first_ok_still_short_circuits_happy_path`
    — happy-path regression pin.
  - `short_circuits_on_transport_error_accessor_reflects_state`
    — accessor + idempotence.

- **Phase 26.B — Python facade
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)).**
  `ChainedAuth.with_short_circuit_on_transport_error()` mirrors
  the Rust builder. Returns a NEW `ChainedAuth` (immutable-
  builder cadence). `short_circuits_on_transport_error() ->
  bool` accessor exposes the flag.

  `tako.compat` module docstring updated to mention the new
  builder + the operator-UX rationale. New
  [`tests/python/test_phase26_chained_short_circuit.py`](tests/python/test_phase26_chained_short_circuit.py)
  covers facade attribute presence, immutable-builder semantics,
  idempotence, child preservation across the flag flip, and the
  Phase-21 default-falls-through regression pin from the Python
  side.

### Changed

- `ChainedAuthResolver` gains a `short_circuit_on_transport_error:
  bool` field (private; default `false`). The Phase 21 default
  fall-through-on-any-Err semantics are preserved byte-for-byte
  for callers who don't opt in.
- Workspace + Python crate version bumped to v0.27.0.

### Carried forward to Phase 27+

- Broader infrastructure-error short-circuit (RateLimited /
  CircuitOpen / BudgetExhausted / Provider source-error) —
  warrants per-variant analysis. Phase 27+ may add
  `with_short_circuit_on_infrastructure_errors`.
- OIDC mTLS end-to-end integration test — real TLS server
  requiring client auth (axum-server + rustls + per-test CA);
  ~300 lines of test infra.
- OIDC mTLS cert / key rotation — long-running deployments
  rotating client certs would need a refresh mechanism.
- URL-source images for Bedrock / Ollama (need tako-side
  pre-fetch with SSRF guard), Vertex File API upload flow,
  eval-harness real graders, OIDC refresh-token / revocation.

## [0.26.0] - 2026-05-01

Phase 25 — closes the OIDC introspection auth-method surface to
all six RFC 7662 §2.1 / RFC 8414 / RFC 8705-listed methods tako
ships. Phase 24 added CA-backed mTLS (`tls_client_auth`);
Phase 25 adds the self-signed sibling (`self_signed_tls_client_auth`,
RFC 8705 §2.2). Natural close-out of the ~10-phase OIDC
hardening arc that started with Phase 14.B. Plan:
[plans/PLAN_PHASE25.md](plans/PLAN_PHASE25.md).

The auth-method surface tako now covers:

1. `client_secret_basic` (Phase 15.B.2 default; RFC 7662 §2.1)
2. `client_secret_post` (Phase 16.B.2; RFC 7662 §2.1)
3. `client_secret_jwt` (Phase 17.B; RFC 7521 / 7523 HS256)
4. `private_key_jwt` (Phase 18.A; RFC 7521 / 7523 RS256 / ES256
   / EdDSA)
5. `tls_client_auth` (Phase 24; RFC 8705 §2.1 CA-backed mTLS)
6. `self_signed_tls_client_auth` (Phase 25; RFC 8705 §2.2
   self-signed mTLS)

### Added

- **Phase 25.A — OIDC introspection
  `self_signed_tls_client_auth`
  ([crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs)).**

  New `IntrospectionAuthMethod::SelfSignedTlsClientAuth`
  variant. Wire-identical to Phase 24's
  `IntrospectionAuthMethod::TlsClientAuth` (both present a TLS
  client cert during the handshake), but the issuer matches the
  cert directly against a pre-registered cert thumbprint or
  public-key fingerprint instead of validating against a CA
  chain. The distinction is in the discovery-list entry; the
  wire format is identical so the same `mtls_client` field on
  `IntrospectionConfig` carries the Identity for both variants.

  Two new `OidcAuthResolver` builders:
  - `with_introspection_self_signed_mtls(cert_pem, key_pem)`
  - `with_introspection_self_signed_mtls_combined(combined_pem)`

  Both load the cert + key, build a per-resolver mTLS-enabled
  `reqwest::Client` via `reqwest::Identity::from_pem`, and flip
  `auth_method` to `SelfSignedTlsClientAuth`. PEM parse /
  `Client` build failures surface as `TakoError::Invalid` at
  builder time, matching Phase 24's pattern.

  Auto-selector extension: extends the Phase 24 five-tier
  preference order to a six-tier ordering with
  `tls_client_auth` (CA-backed) at the head and
  `self_signed_tls_client_auth` second. Rationale: CA-backed
  wins because the chain provides ongoing trust validation
  (revocation, etc.). Both gated on having an mTLS identity
  configured. When only `self_signed_tls_client_auth` is
  advertised, the auto-selector picks it (regardless of which
  mTLS builder set the identity up).

  `introspect()` extension: the body-build, Client-swap, and
  no-Authorization-header arms all extend to handle
  `SelfSignedTlsClientAuth` identically to `TlsClientAuth`.
  The pre-flight identity check is generalised over both
  variants, producing distinct error messages
  (`"oidc: tls_client_auth requires mtls_client to be set"` vs.
  `"oidc: self_signed_tls_client_auth requires mtls_client to
  be set"`).

  Six new unit tests:
  - `with_introspection_self_signed_mtls_accepts_valid_pem`
  - `with_introspection_self_signed_mtls_combined_accepts_concatenated_pem`
  - `with_introspection_self_signed_mtls_rejects_garbage_pem`
  - `auto_select_prefers_tls_client_auth_over_self_signed_when_both_listed`
    — even after `with_introspection_mtls` then re-run
    auto-selector with both advertised, CA-backed wins.
  - `auto_select_picks_self_signed_when_only_self_signed_listed` —
    `tls_client_auth` not advertised; auto-selector picks
    self-signed.
  - `introspect_self_signed_tls_client_auth_errors_when_mtls_client_missing`
    — request-time fail.

  Phase 24 mTLS test cert + key fixtures are reused. All Phase
  24 mTLS tests pass byte-for-byte unchanged.

- **Phase 25.B — Python facade
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)).**
  `OidcAuth.with_introspection_self_signed_mtls(cert_pem,
  key_pem)` and `_combined(combined_pem)` mirror the Rust
  builders. Returns a NEW `OidcAuth`. Raises `ValueError` on
  PEM parse failure.

  `OidcAuth.with_introspection_auth_method(method)` alias
  parser extended with four case-insensitive aliases for the
  new variant: `"self_signed_tls_client_auth"` (RFC 8705 §2.2
  spec name), `"self-signed-tls-client-auth"` (kebab variant),
  `"self_signed_mtls"` and `"self-signed-mtls"`
  (operator-friendly shorthands).

  `tako.compat` module docstring updated with the close-out
  note that the OIDC introspection auth-method surface now
  covers all six published methods. New
  [`tests/python/test_phase25_self_signed_mtls.py`](tests/python/test_phase25_self_signed_mtls.py)
  covers facade attribute presence + the alias-parser entries.

- **`plans/PLAN_PHASE25.md` filename free.** A previous file by this
  name held Phase 2.5's plan because of the `2.5 → 25`
  reading. Renamed to `plans/PLAN_PHASE2_5.md` (underscore variant)
  in this phase so the actual Phase 25 plan can claim the
  natural filename. PLAN.md table row + cross-references in
  `plans/PLAN_PHASE1.md` and `plans/PLAN_PHASE3.md` updated.

### Changed

- `IntrospectionAuthMethod` gains a sixth unit variant
  `SelfSignedTlsClientAuth`. The enum keeps `#[derive(Debug,
  Clone, Copy, Default, PartialEq, Eq)]`; default is unchanged
  (`ClientSecretBasic`).
- The pre-flight mTLS identity check in `introspect()` is
  generalised over both mTLS variants and produces
  variant-specific error messages.
- Workspace + Python crate version bumped to v0.26.0.

### Carried forward to Phase 26+

- OIDC mTLS end-to-end integration test — real TLS server
  requiring client auth (axum-server + rustls + per-test CA);
  ~300 lines of test infra.
- OIDC mTLS cert / key rotation — long-running deployments
  rotating client certs would need a refresh mechanism.
- URL-source images for Bedrock / Ollama (need tako-side
  pre-fetch with SSRF guard), Vertex File API upload flow,
  eval-harness real graders, OIDC refresh-token / revocation,
  ChainedAuth short-circuit-on-transport-error.

## [0.25.0] - 2026-05-01

Phase 24 — closes the OIDC introspection mTLS gap that's been
deferred since Phase 16 with the framing "needs reqwest TLS
feature changes at workspace scope". That framing was wrong:
the existing workspace reqwest features
(`["rustls", "webpki-roots", ...]`) already expose
`reqwest::Identity::from_pem` (verified via a probe compile).
Phase 24 implements RFC 8705 mTLS introspection without any
workspace-level dep change. Plan: [plans/PLAN_PHASE24.md](plans/PLAN_PHASE24.md).

After Phase 24 the OIDC introspection auth-method surface
covers all five RFC 7662 §2.1 / RFC 8414-listed methods tako
intends to ship: `client_secret_basic` / `_post` / `_jwt` /
`private_key_jwt` / `tls_client_auth`. RFC 8705 §2.2
`self_signed_tls_client_auth` corner case (issuer accepts
self-signed certs without a CA chain) and end-to-end
mTLS-handshake integration tests (real TLS server requiring
client auth) remain deferred to Phase 25+.

### Added

- **Phase 24.A — OIDC introspection mTLS / `tls_client_auth`
  ([crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs)).**

  New surfaces:
  - `IntrospectionAuthMethod::TlsClientAuth` variant on the
    introspection auth-method enum.
  - `IntrospectionConfig.mtls_client:
    Option<Arc<reqwest::Client>>` field. `Arc` because
    `reqwest::Client` is already internally `Arc`'d; cloning
    is cheap.
  - `OidcAuthResolver::with_introspection_mtls(cert_pem,
    key_pem)` builder. Loads the cert + key, builds a
    per-resolver mTLS-enabled `reqwest::Client` via
    `reqwest::Identity::from_pem`, attaches to the
    introspection config, flips `auth_method` to
    `TlsClientAuth`. PEM parse failure (or `Client` build
    failure) surfaces as
    `TakoError::Invalid("oidc: invalid mTLS identity PEM:
    ...")` at builder time.
  - `OidcAuthResolver::with_introspection_mtls_combined(combined_pem)`
    convenience for the common `cat cert.pem key.pem` shape.
  - Internal helper `build_mtls_identity` that concatenates
    separate cert + key PEMs with a separating newline (which
    is what `reqwest::Identity::from_pem` requires).

  Auto-selector extension: extends the Phase 18.A four-tier
  preference order to a five-tier ordering with
  `tls_client_auth` at the head when (a) the issuer advertises
  it AND (b) an mTLS identity is configured. Rationale: mTLS
  is the strongest authentication method (the private key
  never leaves the client; the cert binds to a DN / SAN the
  issuer pre-registered).

  `introspect()` extension: when `auth_method ==
  TlsClientAuth`, swap to `cfg.mtls_client` (not the resolver's
  default `self.http`) for the POST. Body is credential-free
  (same as Basic) — the issuer authenticates via the TLS
  handshake's client cert, not a body field or `Authorization`
  header. Pre-flight check errors with
  `TakoError::Invalid("oidc: tls_client_auth requires
  mtls_client to be set")` when the auth method was flipped
  without configuring identity (matches the Phase 17.B / 18.A
  request-time-fail pattern for missing JWT keys).

  Seven new unit tests:
  - `with_introspection_mtls_accepts_valid_pem`
  - `with_introspection_mtls_combined_accepts_concatenated_pem`
  - `with_introspection_mtls_rejects_garbage_pem` — PEM parse
    failure surfaces at builder time.
  - `with_introspection_mtls_no_op_without_introspection` —
    chainable-builder cadence (no PEM parsing if no
    introspection config attached yet).
  - `auto_select_prefers_tls_client_auth_when_listed_and_identity_present`
  - `auto_select_skips_tls_client_auth_when_no_identity` —
    falls back to `client_secret_basic` when listed but
    identity missing.
  - `introspect_tls_client_auth_errors_when_mtls_client_missing`
    — request-time fail.

  A 2048-bit RSA self-signed test cert + matching PKCS#8 key
  are embedded as `static &[u8]` PEM fixtures in the test
  module (matches the Phase 18.A pattern).

- **Phase 24.B — OidcAuth mTLS Python facade
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)).**
  `OidcAuth.with_introspection_mtls(cert_pem: bytes, key_pem:
  bytes)` and `with_introspection_mtls_combined(combined_pem:
  bytes)` mirror the Rust builders. Returns a NEW `OidcAuth`.
  Raises `ValueError` on PEM parse failure.

  `OidcAuth.with_introspection_auth_method(method)` alias
  parser extended to accept three case-insensitive aliases for
  the new variant: `"tls_client_auth"` (RFC 8705 spec name),
  `"tls-client-auth"` (kebab variant), `"mtls"`
  (operator-friendly shorthand).

  `tako.compat` module docstring updated. New
  [`tests/python/test_phase24_mtls.py`](tests/python/test_phase24_mtls.py)
  covers facade attribute presence + the alias-parser entries.

### Changed

- `IntrospectionAuthMethod` gains a fifth unit variant
  `TlsClientAuth`. The enum keeps `#[derive(Debug, Clone,
  Copy, Default, PartialEq, Eq)]`; default is unchanged
  (`ClientSecretBasic`).
- `IntrospectionConfig` gains a public `mtls_client:
  Option<Arc<reqwest::Client>>` field; existing struct-literal
  initialisers (test code) need to pass `mtls_client: None`.
- Workspace + Python crate version bumped to v0.25.0.

### Carried forward to Phase 25+

- `self_signed_tls_client_auth` (RFC 8705 §2.2) — issuer
  accepts self-signed certs without a CA chain. Identical wire
  shape to `tls_client_auth`; same builder works, but the
  discovery-list entry is distinct.
- OIDC mTLS end-to-end integration test — real TLS server
  requiring client auth (e.g. rustls-server in the test
  harness) would close the loop.
- OIDC mTLS cert / key rotation — Phase 24 builds the mTLS
  Client once at builder time; long-running deployments that
  rotate client certs would need a refresh mechanism.
- URL-source images for Bedrock / Ollama (need tako-side
  pre-fetch with SSRF guard), Vertex File API upload flow,
  eval-harness real graders, OIDC refresh-token / revocation,
  ChainedAuth short-circuit-on-transport-error.

## [0.24.0] - 2026-05-01

Phase 23 — extends Phase 22's URL-source-image work to Vertex.
After Phase 23 four of the six provider adapters (Anthropic,
OpenAI, Mistral, Vertex) handle outbound URL-source images;
Bedrock + Ollama remain deferred (both need tako-side pre-fetch
with an SSRF guard — different design problem from the
vendor-fetched-URL case). Plan: [plans/PLAN_PHASE23.md](plans/PLAN_PHASE23.md).

### Added

- **Phase 23.A — URL-source images for Vertex via
  `VxPart::FileData`
  ([crates/tako-providers/vertex/src/convert.rs](crates/tako-providers/vertex/src/convert.rs)).**
  Phase 22 framed Vertex's deferral as "Gemini's `fileData`
  accepts only vendor-specific URI schemes (`gs://...`)". Per
  Gemini's published API docs, `fileData` actually accepts URIs
  from three sources:
  - `gs://bucket/path` GCS URIs (Google fetches server-side;
    private buckets need IAM auth on Google's side, not tako's).
  - `https://...` public web URLs — same vendor-fetch security
    posture as Phase 22's Anthropic / OpenAI / Mistral
    URL-source paths.
  - Vertex File API URIs — files uploaded via Google's File
    API (out of scope; needs a separate upload surface that
    tako doesn't expose yet).

  This commit covers the first two. New `VxPart::FileData {
  file_data: VxFileData }` variant on the untagged
  content-part enum; `VxFileData` carries `mime_type` (renamed
  `mimeType` on the wire) and `file_uri` (renamed `fileUri`).
  Camel-case naming matches the existing `inlineData` /
  `functionCall` / `functionResponse` convention.

  Mapping in `message_to_vx`:

  ```rust
  ContentPart::ImageUrl { url, mime } => {
      let Some(mime) = mime else { continue; };
      if !is_supported_vertex_mime(mime) { continue; }
      parts.push(VxPart::FileData {
          file_data: VxFileData {
              mime_type: mime.clone(),
              file_uri: url.clone(),
          },
      });
  }
  ```

  Per Gemini docs `mimeType` is REQUIRED on `fileData` — the
  optional `ContentPart::ImageUrl.mime` is required for the
  Vertex path; mime-less URL-source content silently drops.
  Unsupported MIME types also drop, reusing the
  `is_supported_vertex_mime` filter from Phase 20.A.

  URL-scheme branching: tako does not pre-validate — Gemini
  rejects unsupported schemes at request time. Same pass-through
  pattern as Phase 22.B's choice on Anthropic's `https`-only
  constraint.

  Five new unit tests: `image_url_block_emits_file_data_with_gs_uri`
  (pinned JSON shape with GCS URI),
  `image_url_block_emits_file_data_with_https_uri` (HTTPS URL
  pass-through), `image_url_block_drops_when_mime_missing`,
  `image_url_block_drops_unsupported_mime`,
  `image_url_and_inline_data_can_coexist` (mixed inline base64
  + URL-source parts emit two adjacent `parts` entries —
  `inlineData` + `fileData` — in source order).

### Changed

- `VxPart` enum gains a new `FileData` variant. `#[serde(untagged)]`
  so the addition is wire-invisible to existing
  `inlineData`/`text`/`functionCall`/`functionResponse` paths.
  Phase 20.A inline-data tests pass byte-for-byte unchanged.
- Workspace + Python crate version bumped to v0.24.0.

### Carried forward to Phase 24+

- URL-source images for Bedrock / Ollama. Bedrock's AWS SDK
  `ImageSource` has no URL variant; Ollama's `images` field
  carries bare base64 only. Both need tako-side pre-fetch with
  an SSRF guard — a different design problem from the
  vendor-fetched-URL case Phases 22 + 23 covered.
- Vertex File API upload flow — separate API surface for
  uploading bytes and getting back a Vertex File URI. The
  Phase 23 `VxFileData` part already accepts those URIs, but
  tako doesn't expose an upload helper.
- OIDC introspection mTLS auth methods, OIDC refresh-token /
  revocation-endpoint flows, eval-harness real graders,
  `ChainedAuthResolver` short-circuit-on-transport-error.

## [0.23.0] - 2026-05-01

Phase 22 — closes the long-deferred URL-source-image gap.
Phases 19 + 20 framed the deferral as "server-side fetch needs
a security story", but that concern only applies when *tako*
fetches the URL. The three vendors whose API servers fetch URLs
themselves (Anthropic, OpenAI, Mistral) now accept URL-source
content; the three that would need tako-side pre-fetch (Vertex
file-data, Bedrock, Ollama) stay deferred. Plan:
[plans/PLAN_PHASE22.md](plans/PLAN_PHASE22.md).

### Added

- **Phase 22.A — `tako_core::ContentPart::ImageUrl` variant +
  provider stubs
  ([crates/tako-core/src/types.rs](crates/tako-core/src/types.rs)).**
  New `ContentPart::ImageUrl { url: String, mime: Option<String> }`
  variant. The optional `mime` is a hint some vendors use; others
  ignore it.

  All six provider adapter `convert.rs` files gain exhaustive
  match arms for the new variant; per-vendor disposition:
  - Anthropic: full wiring in 22.B.
  - OpenAI / Mistral: full wiring in 22.C.
  - Vertex: silent-drop (deferred). Gemini's `fileData` accepts
    only vendor-specific URI schemes (`gs://...` GCS, Vertex
    File API URIs); arbitrary `https://` not supported.
  - Bedrock: silent-drop (deferred). The AWS SDK's `ImageSource`
    has no URL variant — would need a tako-side pre-fetch.
  - Ollama: silent-drop (deferred). `images` field carries bare
    base64 only.

  Existing `match`-with-wildcard sites in tako-orchestrator,
  tako-py, tako-compat, and the http-generic provider are
  unaffected (they all use `_ => ...` arms over `ContentPart`).

- **Phase 22.B — Anthropic URL-source via `AnImageSource` enum
  ([crates/tako-providers/anthropic/src/convert.rs](crates/tako-providers/anthropic/src/convert.rs)).**
  `AnImageSource` refactors from a flat struct (with `kind:
  "base64"` literal) to a `#[serde(tag = "type")]`-tagged enum
  with two variants:
  - `Base64 { media_type, data }` — Phase 19.A wire shape, byte-
    for-byte preserved (the literal `kind: "base64"` becomes the
    enum tag). Pinned by the new
    `image_block_base64_wire_shape_unchanged_after_enum_refactor`
    regression test.
  - `Url { url }` — Phase 22.B. Per Anthropic Messages API:
    `{"type": "url", "url": "https://..."}`. No `media_type`
    field — Anthropic's URL variant doesn't accept one.

  The optional `mime` from the core `ContentPart::ImageUrl` is
  intentionally dropped — Anthropic's API rejects unknown source
  fields. Phase 22.B does not pre-validate the URL scheme;
  Anthropic rejects non-`https` URLs at the API boundary.

  Four new unit tests including a multi-source-type regression
  pin (`image_url_and_base64_can_coexist_in_one_message`).

- **Phase 22.C — OpenAI + Mistral URL pass-through
  ([crates/tako-providers/openai/src/convert.rs](crates/tako-providers/openai/src/convert.rs),
  [crates/tako-providers/mistral/src/convert.rs](crates/tako-providers/mistral/src/convert.rs)).**
  Both vendors accept `https://` URLs in `image_url.url`
  directly (the same field that holds data-URLs in 19.B / 20.B).
  The adapter passes `url` through verbatim — no `data:` prefix
  wrapping (regression-pinned by `image_url_does_not_get_data_url_wrapped`).
  Optional `mime` intentionally dropped; neither vendor accepts
  a separate mime hint on URL-source blocks.

  Five new unit tests across the two crates. The Phase 19.B /
  20.B `text_only_message_keeps_flat_string_content` regression
  pins still hold byte-for-byte — the URL-source path shares the
  `has_image` guard with the base64 path, so non-vision messages
  keep the flat-string `content` shape.

- **Phase 22.D — Python facade
  ([python/tako/models.py](python/tako/models.py)).**
  Pydantic `ContentPart` model gains an explicit `url: str | None`
  field. The model already had `extra="allow"` so URL-source
  dicts round-tripped through the wheel before this commit, but
  the explicit field gives type checking, IDE completion, and a
  pinned-by-test surface. Eight new Python tests in
  [`tests/python/test_phase22_image_url.py`](tests/python/test_phase22_image_url.py)
  including parameterised coverage for various HTTPS URL forms
  and a mixed `image` + `image_url` regression pin.

### Changed

- `AnImageSource` widens from `pub struct` to
  `pub enum` (provider-internal type; wire shape on the existing
  `Base64` path is byte-for-byte preserved).
- Workspace + Python crate version bumped to v0.23.0.

### Carried forward to Phase 23+

- URL-source images for Vertex / Bedrock / Ollama. Vertex needs
  per-URL-scheme branching (`gs://...`); Bedrock + Ollama need
  the SSRF security story Phase 22 dodged.
- OIDC introspection mTLS auth methods, OIDC refresh-token /
  revocation-endpoint flows, eval-harness real graders,
  `ChainedAuthResolver` short-circuit-on-transport-error.

## [0.22.0] - 2026-05-01

Phase 21 — closes the long-standing operator gap on the
OpenAI-compat HTTP server. `ChainedAuthResolver` lets operators
compose existing `AuthResolver` impls for the common "accept
either OIDC bearer OR API key" pattern. Plan:
[plans/PLAN_PHASE21.md](plans/PLAN_PHASE21.md).

### Added

- **Phase 21.A — `ChainedAuthResolver` composite auth
  ([crates/tako-compat/src/auth/chained.rs](crates/tako-compat/src/auth/chained.rs)).**
  New always-on (no cargo feature gate) `AuthResolver` impl that
  wraps N children and tries them in append order. The first
  child to return `Ok` short-circuits; on all-`Err` the last
  child's error propagates.

  Public API: `tako_compat::ChainedAuthResolver` with builder
  methods `new()`, `then(child: Arc<dyn AuthResolver>)`, plus
  `len()` / `is_empty()` for assertions. `Clone + Debug + Default
  + Send + Sync + 'static`. Re-exported from both
  `auth/mod.rs` and `lib.rs`.

  Semantics: empty chain returns
  `TakoError::Invalid("chained auth: no resolvers configured")`;
  any `Err` from a child falls through to the next (transient
  OIDC transport failures don't strand a static-API-key
  client); on all-`Err` the last child's error propagates.

  Method named `then(child)` not `with(child)` because `with` is
  a Python keyword — `chain.with(...)` would be a SyntaxError on
  the Python facade. `then` matches the JS `Promise.then` and
  Rust `Future` `.then(...)` idiom for sequential composition.

  Eight new unit tests including a `CountingAuth` mock used by
  `chained_first_match_short_circuits` to assert the second
  child is **not** called when the first short-circuits, and
  `chained_can_nest` exercising recursive composition (a chain
  whose child is itself a chain). The `with`→`then` rename
  landed as a fix commit (9a15877) on top of the initial
  Phase-21.A landing (5856683).

- **Phase 21.B — `ChainedAuth` Python facade
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)).**
  `tako.compat.ChainedAuth` is always-on (no `auth-*` cargo
  feature gate) — children themselves carry whatever gates they
  were built under, so a wheel without `auth-oidc` simply can't
  construct an `OidcAuth` to pass to `then(...)`.

  PyO3 surface: `__init__()` (empty chain), `then(child)`
  (immutable-builder append), `__len__()` (number of children).

  The `extract_auth_resolver` helper that downcasts the
  `serve_openai(auth=...)` kwarg gains a fourth always-on
  `cast::<PyChainedAuth>` arm. Recursive composition works
  (a chain containing another chain).

  `tako.compat.ChainedAuth` re-export at
  `python/tako/compat.py` (always-on `getattr` mirroring the
  existing Jwt/Oidc/Vault cadence). Module docstring updated.
  Class registration at `crates/tako-py/src/lib.rs` (no
  `#[cfg(feature = ...)]` gate).

  Six new Python tests in
  [`tests/python/test_phase21_chained_auth.py`](tests/python/test_phase21_chained_auth.py)
  covering attribute presence, empty construction, immutable-
  builder semantics, `__len__` after stacking, garbage-input
  `ValueError`, and recursive self-nesting.

### Changed

- Workspace + Python crate version bumped to v0.22.0.

### Carried forward to Phase 22+

- URL-source images — Anthropic's `source.type = "url"`,
  OpenAI's `image_url.url` with `https://...`, Vertex's
  `file_data` with `file_uri`. Server-side fetch from
  request-supplied URLs needs a security story.
- OIDC introspection mTLS auth methods (`tls_client_auth` /
  `self_signed_tls_client_auth`) — needs reqwest TLS feature
  changes at workspace scope.
- OIDC refresh-token / revocation-endpoint flows.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).
- `ChainedAuthResolver` short-circuit-on-transport-error
  semantics — Phase 21 treats every `Err` as fall-through; if
  patterns emerge for fail-fast on transport errors, Phase 22+
  may add `with_short_circuit_on_transport_error`.

## [0.21.0] - 2026-05-01

Phase 20 — finishes the vision-content sweep started in Phase 19.
After Phase 20 every shipped provider adapter (Anthropic, OpenAI,
Vertex, Bedrock, Mistral, Ollama — six of six) handles outbound
`ContentPart::Image`. Plan: [plans/PLAN_PHASE20.md](plans/PLAN_PHASE20.md).

### Added

- **Phase 20.A — Outbound image content for Vertex (Gemini)
  ([crates/tako-providers/vertex/src/convert.rs](crates/tako-providers/vertex/src/convert.rs)).**
  New `VxPart::InlineData { inline_data: VxInlineData }` variant
  on the untagged content-part enum. `VxInlineData` carries
  `mime_type` (renamed `mimeType` on the wire — matches the
  existing `functionCall` / `functionResponse` camelCase
  convention) and `data` (raw base64 with any data-URL prefix
  stripped). Per Gemini REST docs:
  `{"inlineData": {"mimeType": "image/jpeg", "data": "<base64>"}}`.

  `is_supported_vertex_mime` filters to the same four MIME types
  as Phase 19 (`image/jpeg`, `image/png`, `image/gif`,
  `image/webp`); other types are silently dropped.
  `strip_data_url_prefix` is a per-crate copy of the Phase-19
  helper, kept per-crate per ARCHITECTURE.md hard rules.

  Five new unit tests covering the pinned JSON shape, data-URL
  prefix stripping, unsupported-MIME silent-drop, the
  supported-MIME matrix, and `strip_data_url_prefix` idempotence.
  URL-source images via `file_data` remain deferred — server-side
  fetch from request-supplied URLs has security implications.

- **Phase 20.B — Outbound image content for Mistral
  ([crates/tako-providers/mistral/src/convert.rs](crates/tako-providers/mistral/src/convert.rs)).**
  Mistral's vision-capable models (Pixtral) accept OpenAI-
  compatible content blocks. The wire format mirrors Phase 19.B
  byte-for-byte: array-shaped `content` with `text` and
  `image_url` blocks (nested `{"url": "..."}` form holding a
  data-URL).

  New surfaces: `MiContent` untagged enum
  (`Text(String)` | `Blocks(Vec<MiContentBlock>)`),
  `MiContentBlock` tagged enum, `MiImageUrl` payload struct.
  `MiMessage.content` field type widens from `Option<String>` to
  `Option<MiContent>`.

  `message_to_mi` refactor follows the 19.B pattern: array form
  emitted only when an image is present; non-vision messages
  preserve byte-for-byte wire shape (pinned by
  `text_only_message_keeps_flat_string_content`); tool-result
  messages keep the flat-string shape (pinned by
  `tool_result_message_keeps_flat_string_content`).

  Six new unit tests mirroring 19.B's coverage: regression pins
  + array-form emission + data-URL normalisation +
  unsupported-MIME drop + tool-result shape + supported-MIME
  matrix.

- **Phase 20.C — Outbound image content for Ollama
  ([crates/tako-providers/ollama/src/convert.rs](crates/tako-providers/ollama/src/convert.rs)).**
  Ollama's `/api/chat` endpoint is fundamentally different from
  the content-block protocols of Anthropic / OpenAI / Mistral /
  Vertex: images live alongside `content` as a sibling
  `images: Vec<String>` field on `OlMessage` carrying bare
  base64 (no MIME prefix, no data-URL). `content` stays a flat
  string.

  New `OlMessage.images: Vec<String>` field gated by
  `#[serde(skip_serializing_if = "Vec::is_empty")]` so non-vision
  messages keep byte-for-byte wire-shape compatibility with
  pre-Phase-20 traffic (pinned by
  `text_only_message_omits_images_field`).

  Source order is preserved across multiple images even though
  they live in a sibling field rather than interleaved with
  text (pinned by `multiple_images_preserve_source_order`).
  Ollama doesn't filter MIME — pass the bytes through and let
  the model decide what formats it can decode.

  Five new unit tests: regression pin on `images`-field
  absence, populated-shape assertion, multi-image source-order
  preservation, data-URL prefix stripping, idempotence smoke.

### Changed

- `MiMessage.content` field type widens from `Option<String>` to
  `Option<MiContent>`. `MiContent` is the new top-level public
  type for Mistral message content. Existing callers reading
  this field directly (none in the workspace per `grep`) need
  to match on the enum.
- `OlMessage` gains a new public `images: Vec<String>` field;
  the `skip_serializing_if = "Vec::is_empty"` gate keeps the
  wire shape byte-for-byte identical for non-vision traffic.
- `VxPart` gains a new `InlineData` variant; existing serialised
  wire shape on text / functionCall / functionResponse paths is
  byte-for-byte preserved.
- Workspace + Python crate version bumped to v0.21.0.

### Carried forward to Phase 21+

- URL-source images — Anthropic's `source.type = "url"`,
  OpenAI's `image_url.url` with `https://...`, Vertex's
  `file_data` with `file_uri`. Server-side fetch from
  request-supplied URLs needs a security story.
- OIDC introspection mTLS auth methods, OIDC refresh-token /
  revocation-endpoint flows, composite `AuthResolver`s.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).

## [0.20.0] - 2026-05-01

Phase 19 — closes the long-stale "vision is out of scope for
Phase 1" markers on the two flagship providers. Anthropic +
OpenAI now emit outbound `ContentPart::Image` content;
Bedrock has shipped this since Phase 2.5 (so the three-of-six
adapters now have it). Vertex / Mistral / Ollama stay deferred
to Phase 20+. Plan: [plans/PLAN_PHASE19.md](plans/PLAN_PHASE19.md).

### Added

- **Phase 19.A — Outbound image content for Anthropic
  ([crates/tako-providers/anthropic/src/convert.rs](crates/tako-providers/anthropic/src/convert.rs)).**
  New `AnBlock::Image { source: AnImageSource }` variant on the
  Anthropic content-block enum. `AnImageSource` carries `kind`
  (always `"base64"` in Phase 19), `media_type`, and `data`.
  Per Anthropic Messages API: `{"type": "image", "source":
  {"type": "base64", "media_type": "image/jpeg", "data":
  "<base64>"}}`.

  `is_supported_anthropic_mime` filters to the four MIME types
  Anthropic accepts (`image/jpeg`, `image/png`, `image/gif`,
  `image/webp`); other types are silently dropped to match the
  existing `Text { text: "" } => None` cadence in
  `content_to_blocks`. `strip_data_url_prefix` normalises
  `data:image/...;base64,<data>` inputs to the bare-base64 form
  Anthropic's API requires; idempotent.

  Five new unit tests covering serialised JSON shape, data-URL
  prefix stripping, unsupported-MIME silent-drop,
  `strip_data_url_prefix` idempotence, and the supported-MIME
  matrix. URL-source images (`source.type = "url"`) remain
  deferred — server-side fetch from request-supplied URLs has
  security implications we haven't designed yet.

- **Phase 19.B — Outbound image content for OpenAI
  ([crates/tako-providers/openai/src/convert.rs](crates/tako-providers/openai/src/convert.rs)).**
  OpenAI's Chat Completions API requires `content` to switch
  from a flat string to an array of typed blocks when an image
  is present. The adapter now emits the array form **only when**
  an image content part is present, preserving byte-for-byte
  wire shape on existing non-vision traffic (pinned by the new
  `text_only_message_keeps_flat_string_content` regression test).

  New surfaces:
  - `OaContent` untagged enum: `Text(String)` (Phase 1 default)
    | `Blocks(Vec<OaContentBlock>)` (Phase 19.B array form).
  - `OaContentBlock` tagged enum (`text` / `image_url`).
  - `OaImageUrl` struct holding the data-URL string.
  - `OaMessage.content` field type widens from `Option<String>`
    to `Option<OaContent>`.

  `message_to_oa` walks once, accumulating both `text_parts`
  (for the flat-string fallback) and ordered `blocks` (text +
  image entries in source order — preserves narrative ordering).
  Tool-result messages keep the flat-string shape because
  OpenAI's API doesn't accept array content on `role=tool`
  (pinned by `tool_result_message_keeps_flat_string_content`).

  Like the Anthropic adapter, OpenAI accepts the same four MIME
  types; other types are silently dropped. `build_data_url`
  normalises double-prefixed inputs.

  Seven new unit tests covering wire-shape regression
  (text-only + tool-result), array-form emission, data-URL
  normalisation, unsupported-MIME drop, the supported-MIME
  matrix, and idempotent `build_data_url`.

- **Phase 19.C — Python facade smoke
  ([tests/python/test_phase19_vision.py](tests/python/test_phase19_vision.py)).**
  Pins the Pydantic `ContentPart` mirror's image-field surface:
  `ContentPart(type="image", mime, data_b64)` constructs cleanly,
  serialises to the dict shape the Rust adapters consume, and
  preserves source order in mixed text + image messages. Seven
  new tests including parameterised coverage for the four
  supported MIME types. The Python facade's `messages_from`
  (`tako-py/src/conv.rs`) remains text-only — wiring image
  content through the wheel's ergonomic Python entry points is
  a richer surface for a later phase.

### Changed

- `OaMessage.content` field type widens from `Option<String>` to
  `Option<OaContent>`. `OaContent` is the new top-level public
  type for OpenAI message content. Existing callers that read
  this field directly (none in the workspace per `grep`) need
  to match on the enum.
- `tako-providers-azure-openai` doesn't read `OaMessage.content`
  directly — it depends on `tako-providers-openai` only at the
  public-API level — and its 4 integration tests remain green
  byte-for-byte.
- Stub markers on Vertex / Mistral / Ollama image-content arms
  reframed from "out of scope" to "deferred to Phase 20+", with
  vendor-specific reasons (Vertex's `inline_data` / `file_data`,
  Mistral's model-specific multimodal, Ollama's LLaVA-family
  embedding).
- Workspace + Python crate version bumped to v0.20.0.

### Carried forward to Phase 20+

- Vision / image content for Vertex + Mistral + Ollama. Best
  handled in a single Phase 20 sweep — each has a different
  per-vendor multimodal-content shape.
- URL-source images (Anthropic's `source.type = "url"` /
  OpenAI's `image_url.url` with a `https://...` value).
  Server-side fetch from request-supplied URLs needs a security
  story.
- OIDC introspection mTLS auth methods, OIDC refresh-token /
  revocation-endpoint flows, composite `AuthResolver`s.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).

## [0.19.0] - 2026-05-01

Phase 18 — clears two more OIDC carry-forward items from the Phase
17 holding pen. Strictly additive: asymmetric `private_key_jwt`
introspection auth method per RFC 7521 / 7523 (RS256 / ES256 /
EdDSA), and an OIDC Session Management 1.0 end-session endpoint
helper. Python facade mirrors both. Plan:
[plans/PLAN_PHASE18.md](plans/PLAN_PHASE18.md).

### Added

- **Phase 18.A — `private_key_jwt` OIDC introspection auth method
  ([crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs)).**
  Asymmetric sibling of Phase 17.B's `client_secret_jwt`. New
  `IntrospectionAuthMethod::PrivateKeyJwt` variant signs the same
  RFC 7521 / 7523 client-assertion JWT but with an RSA / EC /
  Ed25519 private key instead of the symmetric `client_secret`.
  Same wire shape: form-body
  `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`
  + `client_assertion=<jwt>`, no `Authorization` header.

  New surfaces:
  - `ClientAssertionKey` struct holding the algorithm +
    `EncodingKey`. Three typed PEM constructors:
    `from_rs256_pem`, `from_es256_pem`, `from_ed25519_pem`.
    `Debug` impl redacts the key body but exposes the algorithm.
  - `IntrospectionConfig.client_assertion_key:
    Option<Arc<ClientAssertionKey>>`. `Arc` because `EncodingKey`
    doesn't impl `Clone` and `OidcAuthResolver` is `Clone` for
    the Python immutable-builder pattern.
  - `OidcAuthResolver::with_introspection_private_key(key)` —
    attach a key without flipping the auth method.
  - `with_introspection_jwt_rs256_pem(pem)` /
    `with_introspection_jwt_es256_pem(pem)` /
    `with_introspection_jwt_ed25519_pem(pem)` — convenience
    combos that load the PEM AND flip `auth_method` to
    `PrivateKeyJwt`.

  The 17.A auto-selector is extended to a four-tier preference
  order: `private_key_jwt` (only when an asymmetric key is
  configured) > `client_secret_jwt` (only when a symmetric secret
  is configured) > `client_secret_basic` > `client_secret_post`.
  The fail-closed branch now fires only when the issuer
  advertises methods deferred to Phase 19+ (`tls_client_auth`)
  or unknown.

  `introspect()` refactored:
  - Existing `build_client_assertion_hs256(client_id,
    client_secret, audience)` renamed to
    `build_client_assertion(client_id, audience, &EncodingKey,
    Algorithm)` — a single signing path used by both
    `ClientSecretJwt` and `PrivateKeyJwt`.
  - `PrivateKeyJwt` errors at request time
    (`TakoError::Invalid("oidc: private_key_jwt requires
    client_assertion_key to be set")`) when no key configured.

  Nine new tests including a wiremock test that captures the
  posted body, parses out the `client_assertion` JWT, verifies
  the RS256 signature against the matching public key, and
  asserts the claim layout (`iss` / `sub` = `client_id`, `aud` =
  `introspect_uri`, `exp` ~ 30s in the future). 2048-bit RSA and
  P-256 EC test keypairs embedded as `static` PEM fixtures (test
  use only).

- **Phase 18.B — OIDC Session Management 1.0 end-session
  endpoint helper
  ([crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs)).**
  The OIDC Session Management 1.0 spec defines `end_session_endpoint`
  as a discovery-doc field (§2.2.1) and a query-string-formatted
  URL for relying-party-initiated logout (§5).
  `DiscoveryDoc.end_session_endpoint:
  Option<String>` is now captured at construction time, threaded
  into a new private
  `OidcAuthResolver.discovered_end_session_uri: Option<String>`.

  Two new public methods:
  - `OidcAuthResolver::end_session_endpoint() -> Option<&str>` —
    returns the captured URI; `None` when the issuer doesn't
    implement OIDC Session Management.
  - `OidcAuthResolver::build_logout_uri(id_token_hint,
    post_logout_redirect_uri, state) -> Option<String>` — builds
    the redirect URL per the spec. All params optional. Returns
    `None` when the issuer didn't advertise the endpoint. URL-
    encodes via `url::form_urlencoded::Serializer`. Joins with
    `?` or `&` depending on whether the configured endpoint
    already carries a query string (RFC 3986 conformance).

  Pure URL building; no I/O. Seven new tests covering discovery-
  doc parsing, accessor plumbing, the bare-endpoint case, all-
  params formatting (with URL-encoded `post_logout_redirect_uri`
  containing `https%3A%2F%2F...`), the existing-query-string
  separator semantics, and partial-param sets.

- **Phase 18.C — Python facade mirror
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)).**
  `tako.compat.OidcAuth` gains:
  - `with_introspection_jwt_rs256_pem(pem: bytes)` /
    `with_introspection_jwt_es256_pem(pem: bytes)` /
    `with_introspection_jwt_ed25519_pem(pem: bytes)` — load an
    asymmetric private-key PEM and switch the introspection auth
    method to `private_key_jwt`. Raises `ValueError` on PEM
    parse failure.
  - `with_introspection_auth_method(method)` alias parser
    extended to accept case-insensitive `"private_key_jwt"` /
    `"private-key-jwt"` (in addition to the existing aliases for
    Basic / Post / `client_secret_jwt`).
  - `end_session_endpoint() -> Optional[str]` — returns the
    captured URI.
  - `build_logout_uri(id_token_hint=None,
    post_logout_redirect_uri=None, state=None) -> Optional[str]` —
    the URL-builder helper.

  `tako.compat` module docstring updated to mention the new
  entry points. New
  [`tests/python/test_phase18_oidc.py`](tests/python/test_phase18_oidc.py)
  covers facade attribute presence; the 16 new Rust unit tests
  across 18.A + 18.B remain the source of truth for behaviour.

### Changed

- `IntrospectionAuthMethod` gains a fourth unit variant
  `PrivateKeyJwt`. The enum keeps
  `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]` — all
  variants are unit-shaped, no field-bearing variants. The
  default remains `ClientSecretBasic`; existing wire shape on
  Basic / Post / `client_secret_jwt` paths is byte-for-byte
  preserved.
- `IntrospectionConfig` gains a public `client_assertion_key:
  Option<Arc<ClientAssertionKey>>` field; existing struct-literal
  initialisers (test code) need to pass the new field
  (`client_assertion_key: None`).
- `OidcAuthResolver` gains a private `discovered_end_session_uri:
  Option<String>` field; existing struct-literal initialisers
  (test code) need to pass the new field
  (`discovered_end_session_uri: None`). The runtime constructor
  (`discover()`) auto-populates from the discovery doc.
- The internal `build_client_assertion_hs256` helper is renamed
  to `build_client_assertion` and accepts `&EncodingKey` +
  `Algorithm` directly. Both JWT auth-method variants share the
  same signing path. Module-private; no public-API impact.
- Workspace + Python crate version bumped to v0.19.0.

### Carried forward to Phase 19+

- OIDC introspection mTLS (`tls_client_auth` /
  `self_signed_tls_client_auth`) auth methods — needs reqwest
  TLS feature changes at workspace scope.
- OIDC refresh-token / revocation-endpoint (RFC 7009) flows —
  tako as token consumer rather than validator (different model
  from the existing `AuthResolver` surface).
- Composite `AuthResolver`s (mTLS + bearer chaining).
- Vision / image content support across Anthropic / Vertex /
  Bedrock.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).

## [0.18.0] - 2026-05-01

Phase 17 — closes the two OIDC introspection auth-method items
that Phase 16.B.2 explicitly deferred. Strictly additive:
discovery-driven auth-method selection per RFC 8414 reads the
`introspection_endpoint_auth_methods_supported` field of the
issuer's discovery doc and auto-selects the strongest method;
`client_secret_jwt` introspection auth method per RFC 7521 / 7523
builds an HS256-signed client-assertion JWT and sends it as
`client_assertion` form fields. Python facade mirrors both.
Plan: [plans/PLAN_PHASE17.md](plans/PLAN_PHASE17.md).

### Added

- **Phase 17.A — Discovery-driven OIDC introspection auth-method
  selection
  ([crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs)).**
  The OIDC discovery doc (RFC 8414) advertises supported
  introspection-endpoint auth methods via the
  `introspection_endpoint_auth_methods_supported` field.
  `OidcAuthResolver` now captures that list at construction time
  in a new `discovered_introspection_auth_methods: Option<Vec<String>>`
  field. The new chainable
  `with_introspection_auth_method_from_discovery()` builder picks
  the strongest mutually-supported method per the preference
  order shipped in 17.B (`client_secret_jwt` >
  `client_secret_basic` > `client_secret_post`).
  - Silent no-op when no introspection config has been attached
    yet (matches the Phase-16.B.2 chainable-builder cadence).
  - When discovery did not advertise the field (`None`): selects
    `ClientSecretBasic` per RFC 8414's documented default.
  - When discovery advertised a list with **no** supported
    variant (e.g. issuer requires only `tls_client_auth` or
    `private_key_jwt`, both deferred to Phase 18+): returns
    `TakoError::Invalid("oidc: no supported introspection auth
    method advertised by issuer; supported: [...]")`. Surfacing
    this at builder time rather than as an HTTP-401 from the
    introspection endpoint helps the operator notice the
    misconfiguration before the first request.
  Six new unit tests covering discovery-doc parsing, the no-op /
  field-absent / Basic-listed / Post-only-listed / fail-closed /
  Basic-preferred-over-Post cases. Existing 15.B.2 / 16.B.2 wire
  tests still byte-for-byte green.

- **Phase 17.B — OIDC introspection `client_secret_jwt` auth
  method
  ([crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs)).**
  New `IntrospectionAuthMethod::ClientSecretJwt` variant per
  RFC 7521 / 7523 client-assertion JWT authentication. When
  selected, `introspect()` builds a short-lived HS256 JWT signed
  over the configured `client_secret` and sends it as the
  `client_assertion` form field alongside the fixed
  `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`.
  No `Authorization` header is sent.

  JWT claims (RFC 7521 §5 / 7523 §3 / 7519): `iss` = `sub` =
  `client_id`; `aud` = `introspect_uri` (binds the assertion to
  its target endpoint to prevent replay against a different
  endpoint at the same authorization server); `iat` = unix-now;
  `exp` = `iat + 30s` (RFC 7521 §4.2 recommends a "short
  lifetime"); `jti` = `{nanos}-{counter}` from a process-monotonic
  `AtomicU64` paired with the wall-clock nanosecond — RFC 7519
  §4.1.7 only requires uniqueness within the issuer's tokens, the
  combo gives effectively-zero collision risk inside the
  30-second validity window even across process restarts.

  Errors at request time (not builder time) when `ClientSecretJwt`
  is selected but `client_secret.is_none()` — HS256 needs the
  symmetric key. The 17.A auto-selector is extended to prefer
  `client_secret_jwt` when (a) the issuer advertises it AND (b)
  a `client_secret` is configured; new `auto_select_skips_jwt_when_no_secret`
  test pins this guardrail.

  Asymmetric `private_key_jwt` (RS256 / ES256 with separate
  signing-key storage) remains deferred to Phase 18+ —
  `EncodingKey` doesn't impl `Clone` cleanly and a separate
  config surface is warranted.

  Seven new tests: `auto_select_prefers_jwt_when_listed_and_secret_present`,
  `auto_select_skips_jwt_when_no_secret`,
  `auto_select_errors_when_jwt_only_listed_and_no_secret`,
  `introspect_jwt_errors_when_secret_missing`,
  `introspect_jwt_carries_client_assertion_form_fields`
  (asserts the form-encoded `client_assertion_type=urn%3A...` and
  `client_assertion=...` fields, AND the absence of a
  `client_secret=` field), `introspect_jwt_signed_with_client_secret_hs256`
  (captures the wiremock request body, parses out the
  `client_assertion` JWT, verifies the HS256 signature against
  the configured client_secret using `jsonwebtoken::decode`, and
  asserts the claim layout), and `make_jti_yields_unique_values`
  (256-iter uniqueness smoke).

- **Phase 17.C — Python facade mirror
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)).**
  `tako.compat.OidcAuth.with_introspection_auth_method` alias
  parser extended to accept case-insensitive `"jwt"` /
  `"client_secret_jwt"`. New chainable
  `tako.compat.OidcAuth.with_introspection_auth_method_from_discovery()`
  instance method. Module docstring at
  [`python/tako/compat.py`](python/tako/compat.py) updated to
  mention the new entry points. New
  [`tests/python/test_phase17_oidc.py`](tests/python/test_phase17_oidc.py)
  covers facade attribute presence; the eight new Rust unit
  tests across 17.A + 17.B remain the source of truth for
  behaviour.

### Changed

- `IntrospectionAuthMethod` gains a third unit variant
  `ClientSecretJwt`. The enum keeps
  `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]` — the
  new variant is unit-shaped so all 16.B.2 Copy/Eq machinery
  still works. Default remains `ClientSecretBasic`; existing
  Basic / Post wire shape byte-for-byte preserved.
- `OidcAuthResolver` gains a private
  `discovered_introspection_auth_methods: Option<Vec<String>>`
  field; existing struct-literal initialisers (test code) need
  to pass the new field. The runtime constructor (`discover()`)
  remains the only public path so this isn't a breaking change
  for external callers.
- Workspace + Python crate version bumped to v0.18.0.

### Carried forward to Phase 18+

- OIDC introspection mTLS (`tls_client_auth` /
  `self_signed_tls_client_auth`) auth methods — needs client TLS
  material plumbed through `reqwest::ClientBuilder`.
- OIDC introspection `private_key_jwt` (asymmetric JWT client
  auth — RS256 / ES256 with separate signing-key storage).
- OIDC refresh-token / end-session endpoint flows.
- Composite `AuthResolver`s (mTLS + bearer chaining).
- Vision / image content support across Anthropic / Vertex /
  Bedrock.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).

## [0.17.0] - 2026-05-01

Phase 16 — production hardening of the streaming-verifier and auth
surfaces shipped in Phases 13–15. Strictly additive: bounded mpsc
backpressure in the AB-MCTS / Conductor streaming rollout channels
(closes the unbounded-memory-under-slow-consumer hazard introduced
when streaming verifiers landed); Vault Enterprise namespace
support on `VaultAuthResolver`; OIDC RFC 7662 introspection
`client_secret_post` auth method on `OidcAuthResolver`. Python
facade mirrors the new auth surfaces.
Plan: [plans/PLAN_PHASE16.md](plans/PLAN_PHASE16.md).

### Added

- **Phase 16.A — Bounded mpsc backpressure in streaming verifier
  rollouts.** `AbMcts::stream` and `Conductor::stream` previously
  used `tokio::sync::mpsc::unbounded_channel` for per-delta event
  fanout; a slow downstream consumer (or a slow inline
  `Verifier::evaluate_streaming` impl in Conductor's case) could
  let `OrchEvent`s / `WorkerStreamEvent`s pile up unbounded.

  - **16.A.1 ([crates/tako-orchestrator/src/ab_mcts.rs](crates/tako-orchestrator/src/ab_mcts.rs#L484-L496)).**
    Replace `unbounded_channel::<OrchEvent>()` at line 485 with
    `channel::<OrchEvent>(ROLLOUT_EVENT_BUFFER = 64)`. Producer
    (`rollout_static_streaming`) blocks on `send().await` once
    full; three send sites gain the trailing `.await`. Magic
    number matches the existing
    [`tako-mcp/src/transport/grpc.rs`](crates/tako-mcp/src/transport/grpc.rs#L45-L46)
    precedent. New regression test
    `ab_mcts_stream_bounded_backpressure_high_delta_count` drives
    256 deltas through the 64-slot channel under a counting
    streaming verifier.
  - **16.A.2 ([crates/tako-orchestrator/src/conductor.rs](crates/tako-orchestrator/src/conductor.rs#L543)).**
    Same swap on the `WorkerStreamEvent` channel — `dispatch_workers_streaming`
    and `run_one_worker_streaming` signatures change from
    `mpsc::UnboundedSender` to `mpsc::Sender`. New regression test
    `conductor_stream_bounded_backpressure_high_delta_count`.
  - **16.A.3 — Trinity unchanged.** `Trinity::stream` calls
    `evaluate_streaming` inline on the provider stream (no channel,
    no fanout) — already serial, no plumbing needed.

- **Phase 16.B.1 — Vault Enterprise namespace support
  ([crates/tako-compat/src/auth/vault.rs](crates/tako-compat/src/auth/vault.rs)).**
  `VaultAuthResolver` gains a `namespace: Option<String>` field
  and a chainable `with_namespace(namespace)` builder method. The
  value is threaded through
  [`VaultClientSettingsBuilder::namespace`](https://docs.rs/vaultrs/0.7/vaultrs/client/struct.VaultClientSettingsBuilder.html)
  in `get_or_build_client` so each cached `VaultClient` sends the
  `X-Vault-Namespace` header on every KV lookup. Chainable on top
  of `new` / `with_provider` / `with_approle` / `with_kubernetes` /
  `with_kubernetes_in_pod` — namespace is orthogonal to auth
  method. `None` (default) preserves OSS-Vault byte-for-byte.
  Four new unit tests.

- **Phase 16.B.2 — OIDC introspection `client_secret_post` auth
  method
  ([crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs)).**
  Phase 15.B.2 shipped RFC 7662 token introspection with HTTP
  Basic auth only. Phase 16.B.2 adds a sibling auth method per
  RFC 7662 §2.1: new public
  `IntrospectionAuthMethod` enum (`#[derive(Default)]`, default
  variant `ClientSecretBasic`) with a `ClientSecretPost`
  alternative. `IntrospectionConfig` gains an `auth_method` field;
  `OidcAuthResolver::with_introspection_auth_method(method)` is a
  chainable post-`with_introspection*` setter. `introspect()`
  branches on `auth_method`: `Basic` keeps the
  `Authorization: Basic` header; `Post` adds `client_id` /
  `client_secret` form fields and omits the header. Discovery-
  driven selection (RFC 8414
  `introspection_endpoint_auth_methods_supported`),
  `client_secret_jwt`, and mTLS auth methods remain deferred to
  Phase 17+. Five new wiremock-based unit tests.

- **Phase 16.B.3 — Python facade mirror
  ([crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs)).**
  `tako.compat.VaultAuth.with_namespace(namespace)` and
  `tako.compat.OidcAuth.with_introspection_auth_method(method)`
  expose the new builders. `auth_method` accepts case-insensitive
  `"basic"` / `"client_secret_basic"` / `"post"` /
  `"client_secret_post"` aliases; raises `ValueError` on garbage.
  `#[derive(Clone)]` added to `VaultAuthResolver` so the facade
  can implement the immutable-builder pattern. New
  `tests/python/test_phase16_auth.py` covers the facade surfaces.

### Changed

- `IntrospectionConfig` gains a public `auth_method:
  IntrospectionAuthMethod` field. The default value
  (`ClientSecretBasic`) preserves Phase 15.B.2 wire behaviour
  byte-for-byte; existing struct-literal initialisers need to
  pass the new field (`auth_method: IntrospectionAuthMethod::default()`).
- `dispatch_workers_streaming` and `run_one_worker_streaming` in
  `tako-orchestrator/src/conductor.rs` change from
  `mpsc::UnboundedSender<WorkerStreamEvent>` to
  `mpsc::Sender<WorkerStreamEvent>`. Both functions are
  module-private; no public-API impact.
- `rollout_static_streaming` in `tako-orchestrator/src/ab_mcts.rs`
  changes from `mpsc::UnboundedSender<OrchEvent>` to
  `mpsc::Sender<OrchEvent>`. Module-private; no public-API impact.
- `VaultAuthResolver` now derives `Clone` to support the Python
  immutable-builder pattern (`PyVaultAuth.with_namespace`).

### Carried forward to Phase 17+

- OIDC `client_secret_jwt` and mTLS (`tls_client_auth`)
  introspection auth methods.
- Discovery-driven `introspection_endpoint_auth_methods_supported`
  selection.
- OIDC refresh-token / end-session endpoint flows.
- Composite `AuthResolver`s (mTLS + bearer chaining).
- Vision / image content support across Anthropic / Vertex / Bedrock.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).

## [0.16.0] - 2026-05-01

Phase 15 — clears three more carry-forward items from the Phase 14
holding pen. Strictly additive: streaming-aware `Verifier` in
`AbMcts::stream` (closing the streaming-verifier triumvirate after
Trinity in 13.B and Conductor in 14.A); Vault dynamic token rotation
(AppRole / Kubernetes auth methods) for `VaultAuthResolver`; and
RFC 7662 token introspection for `OidcAuthResolver`. Python facade
mirrors the new auth surfaces.
Plan: [plans/PLAN_PHASE15.md](plans/PLAN_PHASE15.md).

### Added

- **Phase 15.A — Streaming-aware `Verifier` in `AbMcts::stream`.**
  Completes the streaming-verifier triumvirate (Trinity in 13.B →
  Conductor in 14.A → AB-MCTS in 15.A). When the rollout's picked
  provider advertises `Capabilities::supports_streaming` (Phase 9.D
  router-driven mode honoured: capability is checked on the **picked**
  candidate, not the primary), every provider turn in the rollout
  goes through `provider.stream(...)` — tool-call turns assemble
  `ToolCallDelta`s into `ContentPart::ToolCall` exactly like Trinity,
  the final turn produces only `ContentPart::Text`. A cumulative
  text buffer spans the entire rollout; on each non-empty
  `ChatChunk::Delta` the buffer grows and
  `Verifier::evaluate_streaming(&principal, &cumulative)` is called.
  `Ok(Some(score))` returns produce intermediate
  `OrchEvent::VerifierScore { step, branch=leaf_idx, score }` events
  on the same `(step, branch)` as the eventual synthesis-complete
  final. The default `Ok(None)` impl preserves Phase 8 behaviour
  byte-for-byte. Non-streaming providers (and routed candidates
  lacking streaming capability) still produce one full-text
  `OrchEvent::AssistantText` and one final per rollout — identical
  to v0.15.0. Branch identity = `leaf_idx as u32`, computed before
  the leaf is pushed so partials and the final share `(step, branch)`.
  New helper `rollout_static_streaming` runs concurrently with the
  outer `try_stream!` block via a `tokio::sync::mpsc::unbounded_channel`
  + `tokio::select!` recv-loop. `AbMcts::run` (non-streaming path)
  is untouched. Files:
  [crates/tako-orchestrator/src/ab_mcts.rs](crates/tako-orchestrator/src/ab_mcts.rs).
  Tests: seven new tests in
  [ab_mcts_streaming_verifier.rs](crates/tako-orchestrator/tests/ab_mcts_streaming_verifier.rs)
  cover (a) per-delta `AssistantText` events in scripted order, (b)
  per-delta `VerifierScore` partial-and-final shape, (c) partials
  share branch with their rollout's final, (d) `AlwaysScore`
  (default `Ok(None)`) zero-partial regression, (e) non-streaming
  fallback byte-parity, (f) router picks streaming candidate, (g)
  router picks non-streaming candidate. No Python facade changes —
  `PyAbMcts.stream` (Phase 8.B) already surfaces `OrchEvent`
  partials through `PyOrchEventStream`; Phase 15.A just populates
  the existing pipe with new event types.
- **Phase 15.B.1 — Vault dynamic token rotation.**
  [`VaultAuthResolver`](crates/tako-compat/src/auth/vault.rs)
  (Phase 14.B) shipped with a static Vault token baked into a single
  `VaultClient` at construction. Phase 15.B.1 abstracts the bearer-
  token-acquisition strategy behind a new public
  [`VaultTokenProvider`](crates/tako-compat/src/auth/vault_token.rs)
  async trait and ships three impls:
  - `StaticVaultToken` — wraps a fixed string (lossless equivalent
    of v0.15.0 behaviour; `VaultAuthResolver::new(addr, token)`
    internally constructs one).
  - `AppRoleTokenProvider` — POSTs `{role_id, secret_id}` to
    `<addr>/v1/auth/approle/login`, parses `auth.client_token` +
    `auth.lease_duration`, re-authenticates lazily at
    `0.9 * lease_duration`.
  - `KubernetesTokenProvider` — reads the SA JWT from a configurable
    path on each (re-)auth so SA-token rotation is picked up; POSTs
    `{role, jwt}` to `<addr>/v1/auth/kubernetes/login`. Same caching
    pattern as AppRole. Constructor is infallible — missing-JWT
    errors surface only when `token()` is actually called, so unit
    tests on dev workstations work without a populated
    `/var/run/secrets/...`. Convenience constructor
    `KubernetesTokenProvider::in_pod` hardcodes the canonical path.

  All providers POST directly via `reqwest` (NOT
  `vaultrs::auth::approle/auth::kubernetes`) so we don't bump the
  `vaultrs 0.7` dep. Internal helper `vault_login` parses the
  standard Vault auth-response JSON shape (`auth.client_token` +
  `auth.lease_duration` u64 seconds).

  `VaultAuthResolver` keeps its v0.15.0 `new(addr, token)` signature
  and gains `with_provider`, `with_approle`, `with_kubernetes`, and
  `with_kubernetes_in_pod` constructors. Internally a bounded LRU
  (4 entries) of `VaultClient`s keyed on Vault-token-string lets the
  resolver build a fresh client per rotation without rebuilding on
  every request. The principal cache (token → Principal, 60s TTL)
  is **orthogonal** to Vault-token rotation — documented in rustdoc
  to forestall confusion. Feature gate updated:
  `vault = ["dep:vaultrs", "dep:reqwest"]`. Python facade gains
  three `VaultAuth.with_*` static-method constructors mirroring the
  Rust API. Files:
  [crates/tako-compat/Cargo.toml](crates/tako-compat/Cargo.toml),
  [crates/tako-compat/src/auth/vault.rs](crates/tako-compat/src/auth/vault.rs),
  [crates/tako-compat/src/auth/vault_token.rs](crates/tako-compat/src/auth/vault_token.rs),
  [crates/tako-compat/src/auth/mod.rs](crates/tako-compat/src/auth/mod.rs),
  [crates/tako-compat/src/lib.rs](crates/tako-compat/src/lib.rs),
  [crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs).
  Tests: 6 new wiremock-driven integration tests in
  [crates/tako-compat/tests/vault_token.rs](crates/tako-compat/tests/vault_token.rs)
  (login response parsing + caching, 5xx propagation, Kubernetes
  JWT-from-file, missing-JWT path → `TakoError::Transport`,
  re-auth after lease expiry, static-token byte-parity).
- **Phase 15.B.2 — OIDC token introspection (RFC 7662).**
  [`OidcAuthResolver`](crates/tako-compat/src/auth/oidc.rs)
  (Phase 14.B) shipped with signature validation only; revoked
  tokens whose signature still verifies passed. Phase 15.B.2 adds
  opt-in RFC 7662 token introspection as a post-signature-validation
  hook. New public `IntrospectionConfig { introspect_uri, client_id,
  client_secret }` struct. `DiscoveryDoc` is extended with an
  optional `introspection_endpoint` field, captured at `discover()`
  time. Two new builders: `with_introspection(client_id,
  client_secret)` (uses the discovered URI; **fail-closed** if the
  issuer didn't advertise an `introspection_endpoint`) and
  `with_introspection_uri(uri, client_id, client_secret)` (explicit
  URI, infallible — bypasses discovery). The introspection POST
  sends `token=<jwt>&token_type_hint=access_token` as URL-encoded
  form data with HTTP Basic auth carrying
  `client_id:client_secret`. Workspace `reqwest` is configured
  without the `urlencoded` feature, so the body is built via
  `url::form_urlencoded::Serializer` (added behind the `oidc`
  feature gate). Response with `active=false` returns
  `TakoError::Invalid("oidc: token revoked (introspection ...)")`.
  `OidcAuthResolver` derives `Clone` (cheap — JWKS cache is
  `Arc<RwLock<...>>` so cloning shares the cache). Python facade
  gains `OidcAuth.with_introspection` / `with_introspection_uri`
  builder methods returning a NEW pyclass instance (immutable
  builder, matching the Rust `mut self -> Result<Self>` shape).
  Files:
  [crates/tako-compat/Cargo.toml](crates/tako-compat/Cargo.toml),
  [crates/tako-compat/src/auth/oidc.rs](crates/tako-compat/src/auth/oidc.rs),
  [crates/tako-compat/src/auth/mod.rs](crates/tako-compat/src/auth/mod.rs),
  [crates/tako-compat/src/lib.rs](crates/tako-compat/src/lib.rs),
  [crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs).
  Tests: 8 new `#[cfg(test)]` unit tests covering builder behaviour,
  fail-closed semantics, basic-auth header construction, active=true
  / active=false / 5xx flows, and discovery-doc parsing. Plus 5 new
  pytest smokes in
  [tests/python/test_phase15_auth_hardening.py](tests/python/test_phase15_auth_hardening.py).

## [0.15.0] - 2026-04-30

Phase 14 — clears two more carry-forward items from the Phase 13
holding pen. Strictly additive: streaming-aware `Verifier` in
`Conductor::stream` mirroring the Phase 13.B Trinity wiring (worker
fanout now drives `provider.stream(...)` for streaming-capable
workers and surfaces intra-worker progress via an mpsc); three new
production-grade `tako-compat::AuthResolver` impls behind cargo
features (`jwt` / `oidc` / `vault`), each mirrored as a Python
pyclass under matching `auth-*` wheel-side features.
Plan: [plans/PLAN_PHASE14.md](plans/PLAN_PHASE14.md).

### Added

- **Phase 14.A — Streaming-aware `Verifier` in `Conductor::stream`.**
  The Phase 13.B `Verifier::evaluate_streaming` default-impl method
  now drives per-delta `OrchEvent::VerifierScore` events out of
  `Conductor::stream` too, on the same `(step, branch=(idx+1))` as
  the existing Phase 10.C synthesis-complete final score. New
  internal `WorkerStreamEvent { Delta, Done }` enum and
  `dispatch_workers_streaming` free function refactor worker
  fanout: each task now branches on
  `provider.capabilities().supports_streaming` and, when true,
  drives `provider.stream(...)` per worker (mirroring the
  Trinity::stream pattern at trinity.rs:494-595), pushing each
  cumulative-text delta as `WorkerStreamEvent::Delta { branch,
  cumulative }` over a `tokio::sync::mpsc::UnboundedSender`. The
  outer `Conductor::stream` recv-loop calls
  `verifier.evaluate_streaming(...)` per delta and emits
  `OrchEvent::VerifierScore` on hits. Non-streaming workers fall
  through to `provider.chat(...)` — zero partials, one final per
  worker (byte-for-byte parity with v0.14.0). The 1-based `branch`
  index is stamped at task construction and travels with the
  worker, so identity is permit-acquisition-order-independent
  (verified by
  `conductor_branch_index_stable_under_concurrent_completion`).
  `Conductor::run` (non-streaming) is untouched —
  `dispatch_workers_static` still drives `provider.chat()`. The
  shipped `RuleBasedVerifier` (Phase 13.B) overrides
  `evaluate_streaming` already, so Conductor users get the cheap
  heuristic per-delta hook for free with no code change. Files:
  [crates/tako-orchestrator/src/conductor.rs](crates/tako-orchestrator/src/conductor.rs).
  Tests: five new tests in
  [conductor_streaming_verifier.rs](crates/tako-orchestrator/tests/conductor_streaming_verifier.rs)
  cover (a) per-delta partials with stable per-worker branch, (b)
  `AlwaysScore` (default `Ok(None)`) one-final-per-worker
  regression, (c) non-streaming-worker zero-partial shape, (d)
  branch identity under concurrent completion, (e) the
  `ToolCallStart`-precedes-`VerifierScore` ordering invariant.
  Plus a Python smoke in
  [tests/python/test_phase14_conductor_streaming_verifier.py](tests/python/test_phase14_conductor_streaming_verifier.py).
- **Phase 14.B — `tako-compat` real auth providers (JWT / OIDC /
  Vault).** Three new
  [`tako_compat::AuthResolver`](crates/tako-compat/src/auth/mod.rs)
  impls beyond the existing Phase 2 `StaticTokens` placeholder,
  each gated behind a new optional cargo feature on tako-compat so
  default builds inherit no new transitive deps:
  - `jwt` →
    [`JwtAuthResolver`](crates/tako-compat/src/auth/jwt.rs)
    (HS256 / RS256 / ES256). Pins the signature algorithm at
    construction time so `alg=none` and HS/RS confusion attacks
    fail closed (verified in
    `hs256_alg_confusion_rejected`). Configurable claim names
    (default `tenant_id` / `sub` / `roles`); optional `aud` / `iss`
    enforcement.
  - `oidc` →
    [`OidcAuthResolver`](crates/tako-compat/src/auth/oidc.rs).
    Async `discover(issuer, audience)` constructor fetches the
    `/.well-known/openid-configuration` doc once and captures the
    `jwks_uri`. JWKS cache is `Arc<RwLock<JwkSet>>`, refreshed
    lazily when stale (`refresh_interval`, default 1h). On
    signature failure / missing `kid`, the resolver
    force-refreshes the JWKS once and retries (the documented
    JWKS-rotation mitigation from `oauth2-rs`). HS* and `alg=none`
    are explicitly rejected; only RSA / ECDSA / PSS / EdDSA pass.
  - `vault` →
    [`VaultAuthResolver`](crates/tako-compat/src/auth/vault.rs).
    Looks up bearer tokens at
    `<mount>/data/<path_prefix>/<token>` (defaults: `secret` /
    `tako/tokens`). Positive lookups cache for `cache_ttl`
    (default 60s); failed lookups are NOT cached (no negative-cache
    amplification for typos / probes). Vault token rotation
    (AppRole, k8s auth) is deferred to Phase 15+.

  Module re-org: `auth.rs` → `auth/` directory with one file per
  impl (`mod.rs` for the trait + re-exports; `static_tokens.rs` for
  the existing `StaticTokens`, verbatim move via `git mv`
  preserving blame). The `AuthResolver` trait surface is unchanged
  — strictly additive.

  **Python facade.** Mirrored as
  [`tako.compat.JwtAuth`](python/tako/compat.py) /
  `tako.compat.OidcAuth` / `tako.compat.VaultAuth`, each gated on
  a matching `auth-*` wheel-side feature. The slim default wheel
  exposes them as `None` (graceful degradation), so
  `import tako.compat` works regardless of feature set.
  [`tako.compat.serve_openai`](python/tako/compat.py) gains an
  `auth=` parameter; passing both `tokens` and `auth` is a
  `ValueError` so operators must pick a single mode. Files:
  [crates/tako-compat/Cargo.toml](crates/tako-compat/Cargo.toml),
  [crates/tako-compat/src/auth/](crates/tako-compat/src/auth/),
  [crates/tako-compat/src/lib.rs](crates/tako-compat/src/lib.rs),
  [crates/tako-py/Cargo.toml](crates/tako-py/Cargo.toml),
  [crates/tako-py/src/lib.rs](crates/tako-py/src/lib.rs),
  [crates/tako-py/src/py_compat.rs](crates/tako-py/src/py_compat.rs),
  [python/tako/compat.py](python/tako/compat.py).

  Tests: 11 new lib tests in `crates/tako-compat/src/auth/{jwt,
  oidc, vault}.rs` cover JWT round-trip, signature mismatch,
  audience mismatch, alg confusion, missing claims, custom claim
  names; OIDC / Vault have constructor + path-derivation smokes
  (full integration tests requiring a running OIDC mock /
  dev-mode Vault are deferred to a follow-on env-gated suite).
  Plus six new pytest smokes in
  [tests/python/test_phase14_compat_auth.py](tests/python/test_phase14_compat_auth.py).

## [0.14.0] - 2026-04-30

Phase 13 — clears two more carry-forward items from the Phase 12
holding pen. Strictly additive: a new public `StateStore` trait
in `tako-governance` (existing `JsonStateStore` callers keep using
the inherent sync surface unchanged); a new optional
`RedisStateStore` for multi-replica deployments (gated behind a new
`redis` cargo feature); a default-impl `Verifier::evaluate_streaming`
method (existing impls inherit `Ok(None)` and behave exactly as
before); per-delta `OrchEvent::VerifierScore` emission in
`Trinity::stream` when a verifier opts in.
Plan: [plans/PLAN_PHASE13.md](plans/PLAN_PHASE13.md).

### Added

- **Phase 13.A — `StateStore` trait + `RedisStateStore` for
  multi-replica deployments.** New public async trait
  `tako_governance::sigstore_state::StateStore` with required
  `load` / `save` and default-impl `seed` / `persist`
  convenience methods. `JsonStateStore` (Phase 10.A) implements
  the trait via a thin async-over-sync wrapper; the inherent sync
  surface stays the public Phase 10.A API. New
  `tako_governance::sigstore_state_redis::RedisStateStore` (gated
  behind a new `tako-governance/redis` cargo feature; the existing
  `tako-py/redis` feature now also forwards
  `tako-governance/redis` and implies `sigstore`) keeps a single
  shared `tako:sigstore:rekor_min_tree_size` key in Redis.
  Cross-replica safety lives in a small Lua script enforcing
  monotonic write — the cross-process analogue of
  `KeylessVerifier::rekor_max_tree_size`'s in-process
  `fetch_max` — so a slow replica cannot clobber a higher
  water-mark with a stale value. No TTL: unlike daily-bucketed
  `RedisBudgetBackend`, the Rekor anchor is permanent state.
  Both stores ship as siblings — operator picks based on
  deployment topology; the trait makes them interchangeable
  behind `Arc<dyn StateStore>`. Python facade:
  [tako.sigstore.RedisStateStore](python/tako/sigstore.py)
  exposes async `connect` / `load` / `save` / `seed` / `persist`
  via `pyo3_async_runtimes::tokio::future_into_py`. Files:
  [crates/tako-governance/src/sigstore_state.rs](crates/tako-governance/src/sigstore_state.rs),
  [crates/tako-governance/src/sigstore_state_redis.rs](crates/tako-governance/src/sigstore_state_redis.rs),
  [crates/tako-py/src/py_sigstore.rs](crates/tako-py/src/py_sigstore.rs),
  [python/tako/sigstore.py](python/tako/sigstore.py),
  [python/tako/_native.pyi](python/tako/_native.pyi). Tests:
  trait-bound smoke (`fn assert_state_store<T: StateStore>()`),
  async round-trip on `JsonStateStore`, four `#[ignore]`-gated
  live-Redis integration tests in
  [sigstore_state_redis.rs](crates/tako-governance/src/sigstore_state_redis.rs)
  (round-trip, first-boot, monotonic safety, `with_key` override),
  Python smoke gated on `hasattr(_native, "RedisStateStore")` plus
  three live-Redis tests gated on `TAKO_REDIS_TESTS=1` in
  [tests/python/test_phase13_redis_state_store.py](tests/python/test_phase13_redis_state_store.py).
- **Phase 13.B — Streaming-aware `Verifier` in `Trinity::stream`.**
  `tako_core::Verifier` gains an optional
  `evaluate_streaming(&self, principal, partial) -> Option<f32>`
  default-impl method (default `Ok(None)`) mirroring Phase 9.A's
  [`ConfidenceGuard::evaluate_streaming`](crates/tako-core/src/traits/confidence.rs#L54-L60).
  When attached via the existing `.verifier(...)` builder kwarg,
  `Trinity::stream`'s SSE accumulation loop now calls
  `evaluate_streaming(&principal, &cumulative_text)` after each
  non-empty assistant-text delta and yields an
  `OrchEvent::VerifierScore` on the same `(step, branch)` as the
  eventual synthesis-complete final whenever the hook returns
  `Ok(Some(score))`. The Phase 10.C synthesis-complete
  `VerifierScore` at the end of the turn is unchanged; partial
  events are interleaved before it. Consumers distinguish
  partials from the final by `(step, branch)` repetition.
  Throttling lives inside the user's verifier impl via local state —
  same cost-control philosophy as
  `LlmJudgeGuard::with_streaming_min_chars`, no new builder knobs
  on `Trinity`. **Conductor's worker dispatch is non-streaming
  today, so this hook lands only on Trinity** —
  streaming-aware verifier in Conductor is deferred to a future
  phase that refactors `dispatch_workers` to surface intra-worker
  deltas. Default-impl method preserves byte-for-byte parity for
  verifiers that don't override (`AlwaysScore` plus any downstream
  user impls). The shipped `RuleBasedVerifier` (and its Python
  facade `tako.verifiers.RuleBased`) now overrides
  `evaluate_streaming` so the cheap-heuristic verifier drives the
  new hook out of the box. Files:
  [crates/tako-core/src/traits/verifier.rs](crates/tako-core/src/traits/verifier.rs),
  [crates/tako-orchestrator/src/trinity.rs](crates/tako-orchestrator/src/trinity.rs),
  [crates/tako-orchestrator/src/verifiers.rs](crates/tako-orchestrator/src/verifiers.rs).
  Tests:
  [crates/tako-orchestrator/tests/trinity.rs](crates/tako-orchestrator/tests/trinity.rs)
  module `streaming_verifier_emits` — counting verifier emits
  3 partials + 1 final on a 3-delta `StreamingFake`, cumulative
  buffer length verified, plus a regression that `AlwaysScore`
  (no override) emits exactly the existing single
  synthesis-complete event.
  [crates/tako-orchestrator/src/verifiers.rs](crates/tako-orchestrator/src/verifiers.rs)
  unit test verifies `RuleBasedVerifier::evaluate_streaming`
  emits the same proportional score as `score()` would.
  Python smoke at
  [tests/python/test_phase13_streaming_verifier.py](tests/python/test_phase13_streaming_verifier.py).

### Fixed

- **Stale `0.12.0` version markers in two Python files.**
  [python/tako/__init__.py](python/tako/__init__.py) and
  [tests/python/test_smoke.py](tests/python/test_smoke.py)
  carried `__version__ = "0.12.0"` while the workspace was on
  `0.13.0` — Phase 12 oversight. Phase 13 sweeps both forward to
  `0.14.0` along with the workspace version bump.
- **Stale Phase 11.A schema field in
  [tests/python/test_phase10_state_store.py](tests/python/test_phase10_state_store.py).**
  Test asserted the on-disk JSON shape was
  `{"rekor_min_tree_size": N}`, but Phase 11.A added a
  `"version": 1` schema field for forward-incompat detection.
  Updated the assertion to match the current shape.

## [0.13.0] - 2026-04-30

Phase 12 — clears two long-standing debts surfaced in the Phase 11
close-out. Strictly additive: `notifications()` previously returned
an empty stream and now returns real notifications;
`tako.providers.HttpGeneric` is a brand-new symbol. No existing
public API changes shape.
Plan: [plans/PLAN_PHASE12.md](plans/PLAN_PHASE12.md).

### Added

- **Phase 12.A — MCP Streamable HTTP SSE notifications + session
  lifecycle.** `StreamableHttpTransport::notifications()` now opens a
  long-lived `GET {url}` over `text/event-stream`, parses each
  `data:` line as JSON-RPC via `eventsource_stream::Eventsource`,
  and broadcasts method-bearing frames to subscribers via
  `tokio::sync::broadcast`. The reader is spawned lazily on the
  first `notifications()` call (guarded by an
  `AtomicBool::compare_exchange` so concurrent calls share one
  upstream GET); subsequent calls return fresh `broadcast::Receiver`
  subscriptions to the shared feed. Frames carrying an `id` (POST
  responses returned inline by `request()`) are dropped, never
  double-broadcast. `Mcp-Session-Id` captured on a prior POST is
  attached to the SSE GET header so servers that scope
  subscriptions per session see the correct id. `close()` signals
  the reader via `tokio::sync::Notify`. Closes the Phase 2 promise
  tracked in the PLAN.md backlog at
  [crates/tako-mcp/src/transport/streamable_http.rs](crates/tako-mcp/src/transport/streamable_http.rs).
  Tests:
  [crates/tako-mcp/tests/streamable_http_sse.rs](crates/tako-mcp/tests/streamable_http_sse.rs)
  — four wiremock integration tests covering fan-out order,
  `expect(1)` upstream sharing across multiple subscribers,
  id-bearing-frame filtering, and `Mcp-Session-Id` propagation
  from a prior POST.
- **Phase 12.B — `tako.providers.HttpGeneric` Python facade.** Closes
  the Phase 11.B deferred item. Rust `HttpGenericProvider` shipped
  in Phase 11 with chat + streaming via `StreamConfig::OpenAiSse |
  NdJson`; the PyO3 binding is now wired through
  [crates/tako-py/src/py_http_generic.rs](crates/tako-py/src/py_http_generic.rs)
  (mirrors the `PyBedrock` pattern). `body_template` and
  `stream_config` accept Python dict / list / scalar values and
  convert to `serde_json::Value` via `crate::conv::py_to_json`;
  `StreamConfig` deserialises directly because of its existing
  `#[serde(tag = "kind", rename_all = "snake_case")]` representation
  — no enum-mapping plumbing in PyO3. `PyHttpGeneric` is wired into
  both provider-extraction sites
  ([py_orchestrator.rs](crates/tako-py/src/py_orchestrator.rs) and
  the central `extract_provider` helper in
  [py_conductor.rs](crates/tako-py/src/py_conductor.rs) that flows
  through to Conductor / Trinity / AB-MCTS / SelfCaller). The
  Python wrapper at
  [python/tako/providers.py](python/tako/providers.py) exposes a
  `supports_streaming` property surfacing
  `Capabilities::supports_streaming`. Tests:
  [tests/python/test_http_generic_provider.py](tests/python/test_http_generic_provider.py)
  — six tests covering construction, both `StreamConfig` shapes
  flipping `supports_streaming`, the Rust validator surfacing as
  `ValueError`, unknown `stream_config` kinds rejected by serde,
  and `SingleAgent(provider=HttpGeneric(...))` orchestrator wiring.

## [0.12.0] - 2026-04-30

Phase 11 — Sigstore security hardening (review-driven from
[plans/SECURITY_PHASE10.md](plans/SECURITY_PHASE10.md)) plus the long-standing
`http-generic` provider streaming gap. Strictly additive — no public
API change in the sigstore stack; the `http-generic` change is
gated on a new opt-in `stream_config` field that defaults to `None`.
Plan: [plans/PLAN_PHASE11.md](plans/PLAN_PHASE11.md).

### Security

H1 + H2 + M1–M4 from `plans/SECURITY_PHASE10.md`, plus L2 / L3 / L5
opportunistic. The full review motivates each item.

- **H1 — Race-free Rekor checkpoint freshness-anchor advance.**
  `KeylessVerifier::verify_bundle`'s Phase 9.B advance was a
  three-step `load + compare + fetch_max` triple that opened a
  TOCTOU window under concurrent `Arc<KeylessVerifier>` use. Replaced
  with a `compare_exchange_weak` loop on `Acquire` / `AcqRel` /
  `Acquire` orderings; `set_rekor_min_tree_size` and
  `rekor_max_tree_size` upgraded to `Release` / `Acquire` for
  cross-thread happens-before symmetry. New regression test
  `hardening::multi_threaded_advance_never_observes_rollback`
  spawns 16 tokio tasks against a shared verifier with 32
  interleaved checkpoints (covers L4).
- **H2 — `0o600` mode on the `JsonStateStore` file.** `save` now
  `chmod`s the persisted state file to mode `0o600` after the atomic
  rename on Unix; on Windows the chmod is a no-op and operators
  manage confidentiality via NTFS ACLs on the parent directory.
  Documented on the `JsonStateStore` rustdoc, mirrored into the
  Python facade docstring at
  `python/tako/sigstore.py:JsonStateStore`. Examples
  `23_state_store.py` and the new `28_state_store_hardened.py` both
  set `os.umask(0o077)` so the parent dir lands `0700`. New unit
  test `save_clamps_state_file_to_0o600` (Unix-only).
- **M1 + M4 — Atomic `JsonStateStore::save` via `tempfile::NamedTempFile`.**
  Replaces the deterministic `<file>.tmp` + `fs::write` + `fs::rename`
  triple with `NamedTempFile::new_in(parent).persist(...)`. The
  randomised tmp suffix prevents two concurrent saves on a shared
  `Arc<JsonStateStore>` from colliding; the `Drop` impl auto-removes
  the tmp on the failure path, subsuming M4 (no orphan `.tmp` on
  rename failure). `tempfile = "3"` promoted from dev-dep to
  production dep in `crates/tako-governance/Cargo.toml`. Added
  pre-`persist` `sync_all` so power-loss between the rename and the
  inode flush still leaves a consistent file.
- **M2 — `#[serde(deny_unknown_fields)]` + schema `version` on the
  state file.** Strict-mode rejection of any unknown field plus a
  `version: u32` (v1) discriminator. `load` rejects unrecognised
  versions with an explicit "rebuild from a fresh boot" message;
  legacy v0.11.0 state files (no `version` field) load as v1 via
  `#[serde(default)]`. Four unit-test regressions:
  `unknown_field_is_rejected`, `unsupported_version_is_rejected`,
  `legacy_unversioned_file_loads_as_v1`,
  `save_writes_current_version_field`.
- **M3 — `BasicConstraints: cA=TRUE` + `pathLenConstraint` +
  critical-extension enforcement in `verify_chain`.** At every
  issuer hop, `verify_chain` now parses the `BasicConstraints`
  extension and rejects when it's absent or `cA == FALSE`; enforces
  `pathLenConstraint` against the count of intermediates between the
  issuer and the leaf; rejects any `critical: TRUE` extension whose
  OID is not in the known-handled set
  (`BasicConstraints`, `KeyUsage`, `ExtendedKeyUsage`,
  `SubjectAltName`, `SubjectKeyIdentifier`,
  `AuthorityKeyIdentifier`, plus the two Fulcio OIDC OIDs).
  RFC 5280 §4.2 + §4.2.1.9. Three regression tests:
  `chain_rejects_non_ca_intermediate`,
  `chain_rejects_unknown_critical_extension_on_intermediate`,
  `chain_rejects_path_len_constraint_violation`.
- **L2 — `extract_san_value` iterates the full SAN list.** Renamed
  to `extract_san_values` and now returns every string-form SAN
  (`rfc822Name`, `URI`, `dNSName`); `verify_bundle` runs the
  identity policy against the entire set and accepts when at least
  one SAN matches. The pre-fix code returned the first matching-type
  SAN and would either let an attacker-injected SAN sorted earlier
  in the list win the predicate or hide a legitimate SAN behind it.
  Two regression tests in `mod keyless`:
  `l2_predicate_iterates_all_sans`,
  `l2_no_san_match_rejects_with_full_san_list`.
- **L3 — `BTreeMap`-based canonical SET payload.**
  `verify_rekor_set` builds the canonical JSON via
  `BTreeMap<&'static str, serde_json::Value>` + `serde_json::to_string`
  rather than a hand-rolled `format!` with no input escaping.
  Existing Phase 6.E SET fixtures continue to verify (RFC 7159-
  equivalent), so no behaviour change today, but a future
  `RekorEntry` shape change cannot resurrect a silent injection
  vector.
- **L5 — Doc breadcrumb on `extract_oidc_issuer` v1 branch.** Notes
  the unframed-IA5String assumption matches Fulcio's actual
  encoding and points at the v2 `Ia5StringRef::from_der` fallback as
  the breadcrumb if a CA ever flips to proper DER framing.
  Documentation only.

### Added

- **`tako-providers-http-generic` streaming** (Phase 11.B): closes
  the Phase 2 stale marker that previously returned
  `"http-generic does not support streaming yet"`. Operators set
  `HttpGenericConfig::stream_config` to one of:
  - `StreamConfig::OpenAiSse { content_pointer, finish_reason_pointer,
    usage_pointer }` — OpenAI-compatible SSE; reuses
    `eventsource-stream` (the same parser the OpenAI provider
    relies on); terminates on `data: [DONE]`.
  - `StreamConfig::NdJson { … }` — newline-delimited JSON via
    `LinesCodec` from `tokio-util`; terminates on EOF or on a frame
    whose `finish_reason_pointer` resolves to a non-null string.

  Both variants extract content delta, finish reason, and usage via
  RFC 6901 JSON Pointer, so any endpoint with a structured frame
  shape can be configured without code changes. Defaults match the
  OpenAI layout (`/choices/0/delta/content`,
  `/choices/0/finish_reason`, `/usage`). Tool-call delta extraction
  is intentionally out of scope — operators streaming tool calls
  should use the OpenAI provider's typed parser.

  `Capabilities::supports_streaming` is now derived from
  `stream_config.is_some()` unless an operator-supplied
  `HttpGenericConfig::capabilities` overrides it. Tests: 9 unit
  tests on the new types + 9 wiremock integration tests in
  `crates/tako-providers/http-generic/tests/streaming.rs`.

- **New examples** under `examples/`:
  - `28_state_store_hardened.py` — Phase 10.A `JsonStateStore`
    round-trip showing the v0.12.0 confidentiality posture
    (`umask 0o077`, on-disk `0o600` mode-check on Unix).
  - `23_state_store.py` updated with a one-line `os.umask(0o077)`
    call so the example matches the recommended posture.

### Changed

- `KeylessVerifier::set_rekor_min_tree_size` / `rekor_max_tree_size`
  use `Release` / `Acquire` ordering on the `AtomicU64`. No
  observable behaviour change for single-threaded callers.
- `JsonStateStore::save` writes `{"version": 1, "rekor_min_tree_size": …}`
  rather than the v0.11.0 `{"rekor_min_tree_size": …}`. Old v0.11.0
  files still load (treated as v1 via `#[serde(default)]`); new
  binaries reading a future incompatible schema fail loudly.
- `extract_san_value` is now `extract_san_values` (returns
  `Vec<String>`). The keyless verifier path runs the identity
  predicate against the full SAN list. No public API change — the
  function is `pub(crate)`.

### Deferred

- **Python facade for `HttpGenericProvider`.** The plan included a
  `stream_config=` Python kwarg, but `HttpGenericProvider` has no
  Python facade today (it is configured via Rust code or by
  community-supplied wrappers). Adding the full PyO3 binding is a
  Phase 12 candidate if a concrete consumer asks.

## [0.11.0] - 2026-04-30

Phase 10 — Phase 9 follow-on completeness + cross-orchestrator
verifier scores + Python provider streaming. Closes two follow-ons
from `## [0.10.0]`'s release notes (Rekor freshness persistence;
tool-call lifecycle named SSE events), brings `OrchEvent::VerifierScore`
parity to the two non-AB-MCTS streaming orchestrators, and closes
the long-standing Phase 2 stale marker on Python custom provider
streaming. Plan: [plans/PLAN_PHASE10.md](plans/PLAN_PHASE10.md).

### Added

- **On-disk `JsonStateStore` for Rekor freshness** (Phase 10.A):
  closes the v0.10.0 follow-on flagged in `## [0.10.0]`'s release
  notes. Phase 9.B shipped the in-memory anchor on
  `KeylessVerifier::with_rekor_min_tree_size` /
  `rekor_max_tree_size` but persistence was operator-rolled. Phase
  10.A adds a tiny crash-safe helper:
  - New module
    [crates/tako-governance/src/sigstore_state.rs](crates/tako-governance/src/sigstore_state.rs)
    exporting `JsonStateStore { path }` with `new`, `load` (returns
    `Ok(0)` on missing file — matches the verifier's
    "uninitialised" sentinel), `save` (atomic
    `write-temp-then-rename`), `seed(KeylessVerifier) ->
    KeylessVerifier`, and `persist(&KeylessVerifier)` convenience
    wrappers. Wire schema:
    `{ "rekor_min_tree_size": u64 }`.
  - New `&self` setter on `KeylessVerifier`:
    `set_rekor_min_tree_size(n)`, used by the PyO3 facade so the
    anchor can be applied through an `Arc<KeylessVerifier>` without
    ownership transfer. The original consuming
    `with_rekor_min_tree_size(n)` now delegates to it.
  - PyO3: `tako._native.JsonStateStore` exposes `__init__(path)`,
    `load() -> int`, `save(n: int)`, `seed(verifier) -> verifier`,
    `persist(verifier)`, and a `path()` getter. Forwarded through
    `tako.sigstore.JsonStateStore`. `_native.pyi` stub updated.
  - 5 new Rust unit tests in
    `crates/tako-governance/src/sigstore_state.rs::tests` cover
    round-trip, first-boot zero, `.tmp` non-residue after a
    successful save, missing-parent-dir auto-create, and
    parse-error surfacing. 1 new Rust integration test in
    `crates/tako-governance/tests/sigstore.rs::state_store_seed_persist`
    exercises the full seed → verify → persist cycle against the
    existing checkpoint fixture and a simulated process restart
    that rejects a smaller-tree-size bundle. 5 new Python smoke
    tests in `tests/python/test_phase10_state_store.py`.

- **Named `tako.*` SSE events for tool-call lifecycle**
  (Phase 10.B): closes the second Phase 9 follow-on. Phase 9.C
  emitted `tako.verifier_score` / `tako.recursion` named SSE
  extensions; the same mechanism now covers
  [`OrchEvent::ToolCallStart`](crates/tako-orchestrator/src/types.rs)
  and [`OrchEvent::ToolCallResult`](crates/tako-orchestrator/src/types.rs):
  - `event_to_tako_extensions` at
    [crates/tako-compat/src/sse.rs](crates/tako-compat/src/sse.rs)
    gains two new arms:
    - `ToolCallStart { step, name, id }` →
      `("tako.tool_call_start", "{\"step\":N,\"name\":...,\"id\":...}")`.
      Emitted in addition to the existing OpenAI `tool_calls`
      delta from `event_to_payloads` — OpenAI clients ignore the
      named extension per the SSE spec.
    - `ToolCallResult { step, id, result, is_error }` →
      `("tako.tool_call_result", "{\"step\":N,\"id\":...,\"result\":...,\"is_error\":...}")`.
      Closes the gap where this variant had no OpenAI mapping at
      all (silently dropped) so tako-aware clients now see tool
      results mid-stream with `is_error` propagation.
  - 3 new Rust unit tests in `sse.rs::tests`:
    `tool_call_start_emits_named_tako_extension`,
    `tool_call_result_emits_named_tako_extension`, and
    `tool_call_result_propagates_is_error_true`. The pre-existing
    `opaque_variants_emit_no_tako_extensions` regression is
    narrowed to `AssistantText` + `StepStart` (the variants that
    really do remain extension-less).
  - 1 new Rust integration test
    `stream_emits_tool_call_lifecycle_extensions` in
    `crates/tako-compat/tests/server.rs` runs a `ScriptedOrchestrator`
    that emits `ToolCallStart` then `ToolCallResult`; asserts the
    wire body contains both `event: tako.tool_call_start` and
    `event: tako.tool_call_result` lines, the result payload
    preserves the structured tool result and `is_error: false`,
    the named-start frame precedes the OpenAI `tool_calls` delta
    for the same logical event boundary, and the downstream
    assistant-text + `[DONE]` sentinel emit unchanged.

- **`OrchEvent::VerifierScore` for `Trinity` and `Conductor`**
  (Phase 10.C): the v0.9.0 enum variant has been on the wire since
  Phase 8 but only [`AbMcts`](crates/tako-orchestrator/src/ab_mcts.rs)
  emitted it. Phase 10.C adds optional verifier wiring to both
  remaining streaming orchestrators with `None` defaults, so v0.10.0
  behaviour is byte-for-byte preserved when the kwarg is omitted:
  - **`Trinity`**: new `verifier: Option<Arc<dyn Verifier>>` field
    + `TrinityBuilder::verifier(v)` builder method at
    [crates/tako-orchestrator/src/trinity.rs](crates/tako-orchestrator/src/trinity.rs).
    The streaming path emits one `OrchEvent::VerifierScore` after
    each role's assistant turn completes, with `branch` = the
    role's positional index in `role_order` so consumers can
    attribute the score to the specific role/provider that
    produced the turn.
  - **`Conductor`**: new `verifier: Option<Arc<dyn Verifier>>`
    field + `ConductorBuilder::verifier(v)` builder method at
    [crates/tako-orchestrator/src/conductor.rs](crates/tako-orchestrator/src/conductor.rs).
    The streaming path emits one `VerifierScore` per worker output
    before its result is folded back into the next coordinator
    turn, with `branch` = the 1-based worker dispatch index.
    Failed workers (whose `outcome.is_err()`) are skipped — only
    successful text outputs are scored.
  - Both call `verifier.score(principal, prompt_text, output)` at
    synthesis-complete boundaries (never per-delta); per-delta
    cost-controlled judging remains the `LlmJudgeGuard` opt-in.
    `prompt_text` is derived from `input.messages` using the same
    `filter_map(ContentPart::as_text)…join("\n")` pattern AB-MCTS
    has always used, so verifier inputs are consistent across the
    three orchestrators.
  - PyO3: `tako._native.Trinity.__init__` and
    `tako._native.Conductor.__init__` gain optional `verifier=`
    kwargs accepting any `tako._native.RuleBasedVerifier`.
    Forwarded through `tako.Trinity` / `tako.Conductor` with the
    same `verifier=` kwarg + a TypeError check
    (`"verifier must be a tako.verifiers.* instance"`). The
    `extract_any_verifier` helper in
    [crates/tako-py/src/py_ab_mcts.rs](crates/tako-py/src/py_ab_mcts.rs)
    is promoted to `pub(crate)` so all three orchestrators share
    the validation logic. `_native.pyi` stubs updated.
  - 4 new Rust integration tests (2 per orchestrator) under
    `verifier_emits` sub-mods in
    `crates/tako-orchestrator/tests/{trinity,conductor}.rs`:
    - `trinity_emits_verifier_score_when_attached` — code-prompt
      routes to the `code` role, exactly one `VerifierScore` event
      with `branch=0` and `score=0.6` (matching the `AlwaysScore`
      fixture).
    - `trinity_emits_no_verifier_score_when_unattached` — same
      setup without `.verifier(...)`, zero `VerifierScore` events.
    - `conductor_emits_verifier_score_per_worker` — coordinator
      dispatches three workers in one turn, exactly three
      `VerifierScore` events emit with `branch ∈ {1, 2, 3}` and
      `score=0.4`. All three workers' call counters confirm
      execution.
    - `conductor_emits_no_verifier_score_when_unattached` — same
      setup without `.verifier(...)`, zero `VerifierScore` events.
  - 6 new Python smoke tests across
    `tests/python/test_phase10_{trinity,conductor}_verifier.py`
    cover kwarg acceptance, TypeError on a non-verifier argument,
    and default-no-kwarg construction parity.

- **Python custom provider streaming** (Phase 10.D): closes the
  long-standing v0.2.0 stale marker
  `"Python providers do not yet support streaming"` at
  [crates/tako-py/src/py_python_provider.rs](crates/tako-py/src/py_python_provider.rs).
  Pure-Python providers now stream chunks through the same
  orchestrator pipelines that consume native streaming providers
  (Trinity / SelfCaller / AbMcts):
  - New optional `stream=` kwarg on
    `tako._native.PythonProvider.__init__`. Contract:
    `async def stream(request: dict) -> AsyncIterator[dict]` whose
    yielded dicts deserialise to `tako_core::ChatChunk` via the
    standard `kind`-tagged JSON shape — `{"kind": "delta",
    "text": ...}`, `{"kind": "end", "finish_reason": ...,
    "usage": {...}}`, or `{"kind": "error", "message": ...}`.
  - When `stream=` is supplied the Rust side flips
    `Capabilities::supports_streaming` to `true`, so any
    orchestrator that prefers streaming routes through the new
    streaming path automatically.
  - Implementation drives the Python async iterator via
    `__anext__()` once per chunk: GIL is held only long enough
    to schedule each call (via
    `pyo3_async_runtimes::tokio::into_future`); awaits run with
    the GIL released; deserialisation happens under a fresh GIL
    attach via the existing `crate::conv::py_to_json` helper.
    `StopAsyncIteration` cleanly terminates the stream; other
    Python exceptions become `TakoError::Provider` with the
    underlying message preserved.
  - PyO3: optional `stream=` kwarg added; `_native.pyi` stub
    updated; `tako.providers.PythonProvider` Python facade gains
    the same kwarg with the documented contract in its docstring.
    A new `PythonStream` callable type alias documents the
    expected signature.
  - 5 new Python smoke tests in
    `tests/python/test_phase10_python_streaming.py` cover the
    happy path (two deltas + an end frame round-tripping through
    `SelfCaller.stream` to produce the expected joined text),
    backwards-compatible construction without `stream=`, capability
    flag flip when `stream=` is provided, error propagation from
    inside the async generator, and schema-mismatch detection
    when the yielded dict doesn't match `ChatChunk`.

## [0.10.0] - 2026-04-30

Phase 9 — cost-aware streaming guards + transparency-log freshness +
protocol completeness + router-driven AB-MCTS. Closes the four
"Phase 9 candidate" follow-ups flagged in `## [0.9.0]`'s release
notes. Plan: [plans/PLAN_PHASE9.md](plans/PLAN_PHASE9.md).

### Added

- **Streaming-aware `LlmJudgeGuard`** (Phase 9.A): the v0.9.0
  `LlmJudgeGuard` deliberately kept the default
  `evaluate_streaming → Ok(None)` because per-delta judge calls are
  too costly to make default. Phase 9.A adds an explicit opt-in:
  - Two new builder methods at
    [crates/tako-orchestrator/src/self_caller.rs](crates/tako-orchestrator/src/self_caller.rs) —
    `with_streaming_min_chars(usize)` and
    `with_streaming_every_n(u32)`. Default `min_chars = usize::MAX`
    keeps streaming evaluation disabled (preserves v0.9.0 behaviour).
  - The `evaluate_streaming` override returns `Ok(None)` when
    `partial.len() < min_chars` or when an internal
    `Arc<AtomicU32>` counter says "skip this delta". Otherwise
    runs the same `pre_check → chat → record → parse_confidence`
    body as `evaluate` and returns `Ok(Some(score))`. Counter is
    interior so the trait method stays `&self`-immutable.
  - Refactor: judge-call body lifts into a private `run_judge`
    helper shared by `evaluate` and `evaluate_streaming`.
  - PyO3: `tako._native.LlmJudgeGuard.__init__` gains
    `streaming_min_chars=` and `streaming_every_n=` kwargs;
    forwarded through `tako.guards.LlmJudge`. Type stubs in
    `_native.pyi` updated.
  - 3 new Rust integration tests in
    `crates/tako-orchestrator/tests/self_caller.rs::streaming_judge`:
    opt-in basic flow (single judge call when partial crosses
    threshold), default-no-streaming (zero judge calls), every-N
    counting (six over-threshold partials → 2 judge calls). 2 new
    Python smoke tests in `tests/python/test_phase9_streaming_judge.py`.

- **Rekor checkpoint freshness anchor** (Phase 9.B): closes the
  third leg of the transparency-log story alongside Phase 6's SET
  check, Phase 7's inclusion proof, and Phase 8's checkpoint
  signature. Phase 9 adds a trust-on-first-use guard over the
  checkpoint's `tree_size`:
  - New field on `KeylessVerifier`:
    `rekor_min_tree_size: Arc<AtomicU64>` — high-water mark of the
    largest `tree_size` observed on this verifier instance. After
    each successful checkpoint signature + root-hash check,
    `verify_bundle` asserts `checkpoint.tree_size >= prev` and
    atomically advances the mark via `fetch_max`. A smaller value
    is rejected with `TakoError::Invalid(...)` containing the
    rollback details.
  - Two new public methods at
    [crates/tako-governance/src/sigstore.rs](crates/tako-governance/src/sigstore.rs) —
    `with_rekor_min_tree_size(u64)` (seed from a persisted state
    file at startup) and `rekor_max_tree_size() -> u64` (read back
    after each verify to write out). Persistence layer is
    intentionally out-of-band; verifier itself is in-memory.
  - PyO3: `tako._native.KeylessVerifier.__init__` gains
    `rekor_min_tree_size=` kwarg; new method
    `rekor_max_tree_size()` returns `int`. Forward through
    `tako.sigstore.KeylessVerifier`. Type stubs updated.
  - 3 new Rust tests in
    `crates/tako-governance/tests/sigstore.rs::checkpoint_freshness`:
    monotonic ascent (5 → 7 advances mark), rollback rejected
    (post-10 verify of 5 fails with clear error and leaves mark
    unchanged), seed-enforced-from-construction (post-seed-10
    verify of 5 fails on first observation). 2 Python smoke tests.
    The existing `checkpoint` mod's helpers were promoted to
    `pub(super)` for reuse.

- **Named `tako.*` SSE events for `VerifierScore` + `Recursion`**
  (Phase 9.C): the Phase 8 enum variants had no path to OpenAI-compat
  clients; the wildcard arm in
  [crates/tako-compat/src/sse.rs](crates/tako-compat/src/sse.rs)
  silently dropped them. Phase 9.C wires them to the SSE
  sidechannel that OpenAI clients ignore per the SSE spec
  (unknown `event:` lines):
  - New public function `event_to_tako_extensions(&OrchEvent) ->
    Vec<(&'static str, String)>` returns
    `("tako.verifier_score", json_payload)` for `VerifierScore`
    and `("tako.recursion", json_payload)` for `Recursion`. All
    other variants return `Vec::new()` — keeps the OpenAI mapping
    in `event_to_payloads` pure.
  - The route stream builder at
    [crates/tako-compat/src/routes.rs](crates/tako-compat/src/routes.rs)
    now emits each named extension via
    `SseEvent::default().event(name).data(payload)` BEFORE the
    related OpenAI `data:` chunk for the same `OrchEvent`, so a
    verifier score is observable ahead of any text frame.
  - 3 new unit tests in `sse.rs::tests` covering the
    VerifierScore / Recursion shapes plus a wildcard regression
    (opaque variants emit no extension). 1 new integration test in
    `tests/server.rs::stream_emits_named_tako_extension_for_verifier_score`
    asserting the wire format includes
    `event: tako.verifier_score\ndata: {...}\n\n` ahead of the
    OpenAI assistant-text chunk via a `ScriptedOrchestrator`
    fixture. The OpenAI SDK conformance test continues to pass.

- **AB-MCTS router-driven branch expansion** (Phase 9.D): closes
  the most design-heavy Phase 9 candidate. AB-MCTS held a single
  `provider: Arc<dyn LlmProvider>` and used it for every rollout;
  Phase 9 mirrors the SingleAgent `.candidate(p)` + `.router(r)`
  pattern, with the router running **once per rollout** (per
  branch expansion) — the natural granularity for an MCTS search
  tree.
  - New builder methods on `AbMctsBuilder` at
    [crates/tako-orchestrator/src/ab_mcts.rs](crates/tako-orchestrator/src/ab_mcts.rs):
    `.candidate(Arc<dyn LlmProvider>)` (additional providers the
    router may pick) and `.router(Arc<dyn Router>)`. Without
    `router`, candidates are ignored and every rollout uses the
    primary provider — backwards-compatible v0.9.0 behaviour
    (regression-tested).
  - New free helper `pick_rollout_provider` shared by `iterate`
    (the run path) and `stream`, mirroring Phase 8's
    `rollout_static` extraction pattern. Reuses the existing
    `tako_core::Router` trait verbatim — no new types.
  - PyO3: `tako._native.AbMcts.__init__` gains optional
    `candidates=` (list of provider `Py<PyAny>`) and `router=` (a
    `tako._native.RegexRouter` or `OnnxRouter`). Forward through
    `tako.AbMcts(...)` with type-checking on candidates. Type
    stubs updated.
  - 3 new Rust tests in
    `crates/tako-orchestrator/tests/ab_mcts.rs::branch_routing`:
    a `ToggleRouter` alternates across two providers and both
    counters end > 0; a no-router build leaves the candidate's
    counter at 0; a `FailingRouter`'s `Err(...)` propagates from
    `AbMcts::run`. 3 Python smoke tests covering kwarg acceptance,
    type rejection, and the no-router regression.

### Changed

- **README feature matrix + roadmap brought current to Phase 9**
  (Phase 9.E). Matrix had been stuck at Phase 6 since v0.7.0
  (Phases 7–9 lived only in CHANGELOG/PLAN). Adds Phase 7 / 8 / 9
  columns, a new "Streaming guards" row tracking the
  `evaluate_streaming` surface, and Phase 7 / 8 / 9 bullets in the
  Roadmap section.
- Workspace package version: `0.9.0` → `0.10.0` across
  `Cargo.toml`, `pyproject.toml`,
  `python/tako/__init__.py`,
  `tests/python/test_smoke.py`.
- New per-phase plan doc: `plans/PLAN_PHASE9.md`. PLAN.md index row for
  Phase 9 flipped to `done (2026-04-30)`; Phase 10 candidate stub
  added.

### Notes

- **On-disk `JsonStateStore` for Rekor freshness** is intentionally
  out of scope: the 9.B API surface is forward-compatible with a
  follow-on helper. Operators today seed/persist around
  `rekor_max_tree_size()` from their own state layer.
- **Streaming `tako-compat` extension events for tool-call
  lifecycle** are tracked for Phase 10 — the 9.C plumbing
  trivially generalises but no consumer needs them yet.
- **Per-step routing inside an AB-MCTS rollout** stays out of
  scope — branch-level is the right granularity; per-step would
  silently mask branch routing signals and break the "consistent
  provider state inside one branch" invariant.

## [0.9.0] - 2026-04-29

Phase 8 — search streaming + transparency-log completeness. Closes
the four "out of scope" items flagged in `## [0.8.0]`'s release
notes. Plan: [plans/PLAN_PHASE8.md](plans/PLAN_PHASE8.md).

### Added

- **`OrchEvent::VerifierScore` and `OrchEvent::Recursion` variants**
  (Phase 8.A): two new streaming events landed alongside the
  enum's `#[non_exhaustive]` annotation. `VerifierScore { step,
  branch, score }` is consumed by AB-MCTS native streaming
  (Phase 8.B); `Recursion { depth, confidence }` is consumed by
  the streaming-aware `ConfidenceGuard` path on `SelfCaller`
  (Phase 8.D). Serde tag stays `kind`; new variants serialize
  as `{"kind":"verifier_score", ...}` and `{"kind":"recursion",
  ...}`.
- **`tako._native.OrchEvent` Python wrapper** gains four new
  getters: `branch`, `score`, `depth`, `confidence`. Each
  returns `None` for variants that don't carry the field.
  `kind` accepts the two new strings; `step` returns `Some(_)`
  on `verifier_score`. Type stubs in `_native.pyi` updated.

- **Rekor checkpoint (`SignedNote`) verification** (Phase 8.C):
  the third leg of the transparency-log story alongside the
  v0.7.0 SET check and v0.8.0 inclusion-proof check.
  - New `tako_governance::sigstore::RekorCheckpoint
    { origin, tree_size, root_hash_b64, key_id, signature_b64 }`
    struct. `RekorEntry` gains an optional
    `checkpoint: Option<RekorCheckpoint>` field (serde-default
    `None`, so v0.8.0 bundles deserialize unchanged).
  - New private `verify_rekor_checkpoint(rekor_key, checkpoint,
    expected_root_hex)` runs after the inclusion-proof check
    when both a Rekor key is pinned and the entry carries a
    checkpoint. Reconstructs the canonical signed message
    (`format!("{origin}\n{tree_size}\n{root_hash_b64}\n\n")`),
    verifies the ECDSA-P256 signature against the pinned Rekor
    key, and (when an inclusion proof is also present) asserts
    the checkpoint's `root_hash_b64` decodes to the same bytes
    as the inclusion proof's `root_hash_hex` — anchoring the
    audit path to a tree head the operator can also observe
    out-of-band.
  - 3 new Rust integration tests in
    `crates/tako-governance/tests/sigstore.rs::checkpoint`:
    round-trip with all three Rekor checks (SET + inclusion +
    checkpoint), tampered checkpoint signature rejected, and a
    *clean* root-hash-mismatch case where the checkpoint's
    signature is valid but the root disagrees with the
    inclusion proof.
  - **Implicit-on-when-present.** No new `KeylessVerifier`
    builder method — the same `with_rekor_key` already gates
    SET, inclusion-proof, and now checkpoint verification.
  - No Python facade change required (the field is pure data
    inside the bundle JSON; serde handles it transparently).

- **`tako.AbMcts` Python facade** (Phase 8.B continued): closes the
  v0.5.0 gap — AB-MCTS landed in Rust but had no Python binding.
  - New `tako._native.AbMcts(provider, verifier, *, max_iterations=,
    branching_factor=, max_steps_per_rollout=, temperature=,
    min_confidence=)` pyclass with `run`, `run_sync`, and `stream`
    methods. `stream` returns the existing `PyOrchEventStream` from
    Phase 7.B — the `verifier_score` events from 8.A surface via
    that wrapper's new `branch` and `score` getters.
  - New `tako._native.RuleBasedVerifier(min_chars=, pattern=None)`
    pyclass — the only verifier currently exposed; further verifier
    types (callable adapters, custom score fns) are tracked for
    follow-on releases.
  - Python facade: `tako.AbMcts(...)` and `tako.verifiers.RuleBased`
    (new module). Type stubs in `_native.pyi`.
  - 2 new Python smoke tests in
    `tests/python/test_ab_mcts_stream.py`: end-to-end stream against
    a `PythonProvider`-backed AB-MCTS, and verifier-score event
    branch/score-getter assertions.

- **Native `AbMcts::stream` implementation** (Phase 8.B): replaces
  the Phase 4 stub at `crates/tako-orchestrator/src/ab_mcts.rs:
  315-327` (the only orchestrator's `stream` method that was still
  returning a placeholder error).
  - Per iteration, the stream emits exactly:
    1. `OrchEvent::StepStart { step: iteration }`
    2. `OrchEvent::AssistantText { step, delta: rollout_text }`
       carrying the rollout's full text as a single delta. Per-token
       streaming inside a multi-step rollout is deferred — would
       require threading `provider.stream()` through the in-rollout
       tool-call loop, which is non-trivial and out of scope.
    3. `OrchEvent::VerifierScore { step, branch, score }` (variant
       added in 8.A) carrying the leaf's branch index and verifier
       score on `[0, 1]`.
  - `min_confidence` early-stop short-circuits the loop after the
    rollout that crosses the threshold. The stream terminates with
    exactly one `OrchEvent::Final` constructed from the
    highest-scored leaf, matching `run`'s return value.
  - Refactor: the existing rollout body lifts out of
    `AbMcts::rollout` into a free `rollout_static` function so
    `run` and `stream` share the same simulation loop.
  - 3 new Rust tests in
    `crates/tako-orchestrator/tests/ab_mcts.rs::stream`: 10-event
    happy-path round-trip with `AlwaysScore(0.5)`,
    text-before-score ordering invariant across iterations, and
    `min_confidence` early-stop yielding exactly 4 events
    (StepStart + AssistantText + VerifierScore + Final).

- **Streaming-aware `ConfidenceGuard`** (Phase 8.D): the
  trait at `tako_core::ConfidenceGuard` gains a default method
  `evaluate_streaming(&self, principal, partial: &str) ->
  Result<Option<f32>, TakoError>`. The default impl returns
  `Ok(None)` (skip — keep streaming and evaluate the buffered
  final text), so guards that don't override it behave exactly
  as before.
  - `SelfCaller::stream` now accumulates assistant text deltas
    into a per-iteration buffer and consults
    `evaluate_streaming` after each delta. If the override
    returns `Some(score)` with `score >= self.min_confidence`,
    the inner stream is dropped, an `OrchEvent::Recursion`
    event carrying the score is yielded, and a synthesised
    `OrchEvent::Final` over the accumulated text closes the
    stream. Useful for cheap rule-based heuristics.
  - `RuleBasedGuard` overrides `evaluate_streaming` to return
    `Some(1.0)` when the cumulative partial already passes
    both the length check and (when configured) the regex.
  - `LlmJudgeGuard` deliberately does **not** override the
    streaming method — calling out to a judge provider on
    every delta is a cost disaster. The default `Ok(None)`
    preserves correctness.
  - `SelfCaller::stream` also yields a new
    `OrchEvent::Recursion { depth, confidence }` event at the
    end of every iteration boundary (early-abort or buffered
    evaluation), giving consumers a first-class wire signal
    for recursion progress.
  - 2 new Rust tests in `crates/tako-orchestrator/tests/
    self_caller.rs::streaming_guard`: early-abort against a
    `StreamingFake` provider, and a control case proving the
    default `Ok(None)` path doesn't drop deltas.

### Changed

- **`OrchEvent` is now `#[non_exhaustive]`.** Pre-1.0 minor-bump
  break for downstream Rust consumers that exhaustively match
  on the enum — they need to add a wildcard arm. The Python
  facade is unaffected (the dynamic `kind`-based dispatch
  pattern never matched exhaustively).

## [0.8.0] - 2026-04-29

Phase 7 — production hardening, continued. Closes the two follow-ups
flagged in `## [0.7.0]`'s release notes plus the cosign protobuf-bundle
ergonomics carry-over tracked since v0.6.0.

### Added

- **Rekor inclusion-proof (Merkle audit-path) verification**
  (Phase 7.A): extends the v0.7.0 Rekor SET check.
  - New `tako_governance::sigstore::RekorInclusionProof
    { hashes_hex, tree_size, log_index, root_hash_hex }` struct.
    `RekorEntry` gains an optional `inclusion_proof:
    Option<RekorInclusionProof>` field (serde-default `None`, so
    v0.7.0 bundles deserialize unchanged).
  - New private `verify_rekor_inclusion(entry, proof)` runs after
    the SET check in `verify_bundle` when the entry carries a proof
    and a Rekor key is pinned. Algorithm: RFC 6962 §2.1.1 audit-path
    verification — leaf hash `SHA256(0x00 || canonicalized_body)`,
    internal hash `SHA256(0x01 || left || right)`, walk bottom-up
    per the bit-pattern of `(log_index, tree_size)`, assert the
    final hash equals the pinned `root_hash_hex`.
  - 3 new Rust integration tests in
    `crates/tako-governance/tests/sigstore.rs::inclusion_proof`:
    round-trip against a runtime-built 5-leaf Merkle tree (covers
    both the mid-tree and right-edge audit-path branches), tampered
    audit-path-hash rejected, mutated `root_hash_hex` rejected.
  - No Python facade change required — the proof is pure data
    inside the bundle JSON; serde handles the new field
    automatically.
  - **Out of scope (Phase 8 candidate)**: Rekor checkpoint
    (`SignedNote`) verification — orthogonal to the audit path
    itself.

- **Native `SelfCaller::stream` implementation** (Phase 7.B):
  replaces the Phase 4 stub at `crates/tako-orchestrator/src/
  self_caller.rs:192-202` (the only orchestrator's `stream` method
  that was still returning a placeholder error).
  - Mirrors the `Trinity::stream` pattern: clones owned state up
    front, builds an `async_stream::try_stream!` block. Each
    recursion iteration consumes the inner orchestrator's
    `BoxStream<OrchEvent>`, forwards every event verbatim, and
    intercepts `OrchEvent::Final` for the confidence-guard check.
    Only the last accepted (or max-depth) iteration's `Final` is
    yielded; intermediate `Final` events are absorbed.
  - The `OrchEvent` enum is intentionally left unchanged — the
    implicit signal "more `StepStart` events after a `Final`"
    indicates a guard rejection. A first-class
    `OrchEvent::Recursion { depth, confidence }` variant is tracked
    for Phase 8.
  - 3 new Rust tests in
    `crates/tako-orchestrator/tests/self_caller.rs`:
    pass-through-when-confident, recurse-to-max-depth-when-guard-rejects,
    AssistantText-deltas-arrive-before-Final.

- **First streaming Python entry point**
  (Phase 7.B continued): `tako.SelfCaller.stream(prompt, ...)`
  becomes the project's first async-iteration surface.
  - New `tako._native.OrchEvent` pyclass — read-only wrapper with
    a `kind` getter
    (`"step_start" | "assistant_text" | "tool_call_start" |
    "tool_call_result" | "final"`) and per-variant getters
    (`step`, `delta`, `name`, `id`, `result`, `is_error`, `text`,
    `usage`) returning `None` when the field doesn't apply.
  - New `tako._native.OrchEventStream` pyclass — async-iterable
    (`__aiter__` + async `__anext__`) over a
    `BoxStream<Result<OrchEvent>>`. The stream is parked behind a
    `tokio::sync::Mutex` so the pyclass stays `Send + Sync`.
  - `tako.SelfCaller.stream(...)` returns the stream so callers
    write `async for ev in await sc.stream(prompt): ...`. Type
    stubs added to `_native.pyi`. Future Trinity / SingleAgent
    stream bindings can reuse the shared types verbatim.
  - 2 new Python smoke tests in `test_self_caller_stream.py`.

- **cosign protobuf-bundle adapter** (Phase 7.C):
  `KeylessBundle::from_protobuf_bundle(bytes)` decodes a Sigstore
  protobuf-specs `Bundle` v1 message (the wire format of `cosign
  sign-blob --bundle out.pb`) into the JSON-shaped `KeylessBundle`
  the rest of the verifier pipeline already consumes.
  - Hand-rolled `prost::Message` types in
    `crates/tako-governance/src/cosign_bundle.rs` cover only the
    fields tako consumes. Unknown fields
    (`timestamp_verification_data`, DSSE envelopes, `kind_version`,
    Rekor checkpoints) decode as no-ops since prost ignores
    unknown tags. No `sigstore-protobuf-specs` dep, no `prost-build`
    at compile time, no `protoc` at build time.
  - Field translation: leaf cert from
    `verification_material.x509_certificate_chain.certificates[0]`
    (or `.certificate` on newer cosign builds) → `leaf_cert_pem`;
    chain → `chain_pem`; `message_signature.signature` →
    base64 → `signature_b64`; first `tlog_entries[]` →
    `Some(rekor)` including the inclusion proof from 7.A.
  - Gated behind a new `sigstore-protobuf` Cargo feature
    (depends on the existing `sigstore` feature). Default builds
    gain neither prost nor the new module.
  - 3 unit tests in `sigstore.rs::protobuf_tests`: round-trip,
    single-`certificate` form, missing-signature rejection.

- **Python facade**
  (Phase 7.C continued):
  `tako.sigstore.KeylessVerifier.verify_protobuf_bundle(manifest,
  protobuf_bundle)` — same return shape as `verify_bundle`.
  - Gated behind the new `sigstore-protobuf` feature on `tako-py`
    (forwards to the same feature on `tako-governance`); the Python
    facade raises a clear `AttributeError` when the wheel was built
    without it.
  - 3 new Python smoke tests in
    `test_phase7_sigstore_protobuf.py`.

### Changed

- Workspace package version: `0.7.0` → `0.8.0` across
  `Cargo.toml`, `pyproject.toml`, `python/tako/__init__.py`,
  `tests/python/test_smoke.py`.
- New per-phase plan docs: `plans/PLAN_PHASE1.md` (extracted from PLAN.md
  inline body), `plans/PLAN_PHASE4.md` (retroactive — Phase 4 had no
  per-phase doc), and `plans/PLAN_PHASE7.md` (this phase). PLAN.md slimmed
  to a phase-index table + roadmap.

### Notes

- **Rekor checkpoint** verification (signed note over the tree
  head) remains out of scope — orthogonal to the audit path itself.
  Phase 8 candidate.
- **AB-MCTS native streaming** stays deferred to Phase 8.
- **`OrchEvent::Recursion` variant** — defer until a concrete
  consumer asks for it.
- The Phase 7.B Python streaming surface is intentionally minimal
  (events expose getters, not Python dataclasses; iteration is
  one-shot per stream). Generalising to Trinity / SingleAgent is a
  follow-on PR using the same `PyOrchEvent` /
  `PyOrchEventStream` types.

## [0.7.0] - 2026-04-29

Phase 6 — production hardening, continued. Closes the two follow-ups
flagged in `## [0.6.0]`'s release notes:

### Added

- **`BudgetTracker` wired into `Conductor`, `Trinity`, and
  `LlmJudgeGuard`** (Phase 6.A / 6.B / 6.C): mirrors the v0.6.0
  `SingleAgent` pattern across the remaining provider-call sites.
  - `Conductor::builder().budget(Arc<BudgetTracker>)` instruments
    every coordinator call and every fan-out worker call: each worker
    task runs `pre_check` → `chat` → `record` independently. A
    `BudgetExhausted` from a worker collapses into the worker's
    error outcome and is then surfaced via `fail_fast` if enabled.
  - `Trinity::builder().budget(Arc<BudgetTracker>)` instruments the
    chosen role's chat call in `run` and both the streaming and
    non-streaming paths in `stream`.
  - `LlmJudgeGuard::with_budget(Arc<BudgetTracker>)` instruments the
    judge's own provider call so a `SelfCaller` paired with an
    `LlmJudgeGuard` meters confidence-evaluation usage independently
    of the inner orchestrator's regular execution. `SelfCaller`
    itself does not grow a budget field — its `inner` orchestrator
    already carries one and direct provider calls live only in the
    guard.
  - PyO3: `tako._native.{Conductor, Trinity, LlmJudgeGuard}.__init__`
    gains `budget=` and `budget_backend=` kwargs, all routed through
    `crate::py_runtime::extract_budget_backend`. Same kwargs plumbed
    through to the Python facade in `tako.{Conductor, Trinity}` and
    `tako.guards.LlmJudge`.
  - 6 new Rust tests (3 conductor, 2 trinity, 1 self-caller) +
    3 new Python smoke tests
    (`test_phase6_budget_{conductor,trinity,judge}.py`).
  - New example `examples/19_budget_fanout.py` demonstrating budget
    tracking across a Conductor's coordinator + worker fan-out.

- **Sigstore `KeylessVerifier` chain-of-trust + Rekor SET**
  (Phase 6.D / 6.E):
  - New `tako_governance::sigstore::TrustRoot` struct, loadable
    from concatenated PEM blocks (`from_pem`) or filesystem paths
    (`from_paths`). Holds operator-pinned root + intermediate
    certificates as `Vec<x509_cert::Certificate>`.
  - `KeylessVerifier::with_trust_root(TrustRoot) -> Self` extends
    the v0.6.0 leaf-cert + identity-policy check with a
    chain-of-trust walk: each cert in the bundle's new
    `chain_pem` field is signature-validated against its issuer,
    `notBefore` / `notAfter` are checked, and the chain must
    terminate at one of the pinned roots (max 16 hops).
  - `KeylessBundle` gains two backwards-compatible fields:
    `chain_pem: Option<String>` (intermediate certs) and
    `rekor: Option<RekorEntry>` (transparency-log entry +
    SET-signed metadata). Both serde-default to `None`, so v0.6.0
    bundles deserialize unchanged.
  - `KeylessVerifier::with_rekor_key(&[u8]) -> Result<Self>` pins
    the Rekor public-good ECDSA-P256 key. When set and the bundle
    carries a `rekor` field, `verify_bundle` reconstructs the
    canonical entry JSON (sorted keys, no whitespace) and verifies
    the SET. Inclusion-proof (Merkle) verification is intentionally
    deferred to Phase 7.
  - PyO3: new `tako._native.TrustRoot` pyclass; extended
    `tako._native.KeylessVerifier` with `trust_root=` and
    `rekor_public_key_pem=` kwargs. Python facade adds
    `tako.sigstore.TrustRoot` and the matching kwargs on
    `tako.sigstore.KeylessVerifier`.
  - 4 new Rust tests (2 chain validation cases, 2 Rekor SET cases)
    + 2 new Python smoke tests in
    `tests/python/test_phase6_sigstore_chain.py`.
  - New example `examples/20_sigstore_full_chain.py` running the
    full identity + chain + Rekor pipeline against runtime-minted
    fixtures.

- Implementation uses existing deps (`x509-cert`,
  `sigstore::crypto::CosignVerificationKey`); the `sigstore` crate's
  heavy `verify` feature (with `webbrowser` + `openidconnect`) stays
  out of the dep tree.

### Notes

- `SelfCaller::stream` remains stubbed (Phase 4 carry-over). Native
  streaming is tracked for Phase 7.
- Rekor inclusion-proof (Merkle proof against the log root) is
  intentionally out of scope for v0.7.0. The `RekorEntry` JSON shape
  is forward-compatible with an added `inclusion_proof` field.
- A `cosign-bundle.json → KeylessBundle` shim is still tracked for a
  future ergonomics pass.

## [0.6.0] - 2026-04-29

Phase 5 — production hardening. Closes the three explicit follow-ups
flagged in `## [0.5.0]`'s release notes:

### Added

- **Sigstore keyless verification** (`tako_governance::KeylessVerifier`,
  Phase 5.A): a second trust model alongside the Phase-4 keyed
  `CatalogueVerifier`. The catalogue is signed by a short-lived
  Fulcio-issued leaf certificate that binds the artifact to a specific
  OIDC identity (issuer URI + SAN). Operators pin an `IdentityPolicy
  { issuer, san_match }` (where `SanMatch::Exact` or `SanMatch::Regex`)
  and call `verify_bundle(manifest, bundle)`; the verifier checks the
  cert's `notBefore` / `notAfter`, the Code Signing extended key usage,
  the OIDC issuer extension (`1.3.6.1.4.1.57264.1.1`), the SAN, and the
  signature against the cert's public key. Returns the same
  `Catalogue` shape as the keyed verifier so call sites are
  interchangeable.
- The bundle wire format (`KeylessBundle { leaf_cert_pem,
  signature_b64 }`) is a small JSON wrapper an operator can produce
  from `cosign sign-blob` output in a few lines of shell.
- Trust scope for v0.6.0 is **leaf-cert + identity-policy +
  signature**. Chain-of-trust validation against the Fulcio root and
  Rekor SET / inclusion-proof verification are explicitly deferred —
  the `verify_bundle` return shape is forward-compatible. This
  intentionally avoids the heavy `sigstore` `verify` feature
  (transitively requires `webbrowser` + `openidconnect`).
- `tako-governance` adds direct deps on `x509-cert = "0.2"` (already
  pulled in transitively by `sigstore`), `const-oid = "0.9"`, and
  `pem = "3"`, all gated behind the `sigstore` feature. Test deps add
  `rcgen` (with `aws_lc_rs` + `pem`).
- 6 Rust tests in `crates/tako-governance/tests/sigstore.rs::keyless`
  generate a Fulcio-style leaf cert at runtime (no fixtures committed):
  happy path, regex SAN, wrong issuer, wrong SAN, tampered manifest,
  malformed bundle.

- **gRPC MCP mTLS** (`tako_mcp::GrpcTransport::connect_with_tls`,
  Phase 5.B): a second constructor on the Phase-4 `GrpcTransport`
  alongside the existing plaintext / webpki-roots `connect`. Takes
  `(endpoint, ca_pem, client_cert_pem, client_key_pem, domain_name)`.
  When `client_cert_pem` and `client_key_pem` are both set, the
  transport sends a client certificate (mTLS); pass `None` for both to
  use the custom CA without client auth. Half-pair client identities
  raise synchronously with a clear error. The post-channel demux/spawn
  logic is refactored into a private `from_channel` helper shared by
  both constructors.
- 4 Rust tests in `crates/tako-mcp/tests/grpc.rs::mtls` mint a
  self-signed CA + server cert + client cert at runtime via `rcgen`
  and bind an in-process `tonic::transport::Server` with
  `ServerTlsConfig::client_ca_root`: full mTLS round-trip; server
  rejection without a client cert; CA-only round-trip without client
  auth; eager rejection of half-pair client identity.
- `tako-mcp` gains a tiny dev-dep on `rustls = "0.23"` (with the
  `aws_lc_rs` provider) so the test binary can pin a CryptoProvider —
  both `aws-lc-rs` (via rcgen) and `ring` (via tonic) end up linked,
  and rustls 0.23 refuses to auto-pick when both are present.

- **`BudgetTracker` wired into the SingleAgent orchestrator API**
  (Phase 5.C): closes the regression flagged in `## [0.5.0]` Phase 4.G
  notes. `SingleAgent` and `SingleAgentBuilder` gain an optional
  `Arc<BudgetTracker>` field plus a `.budget(...)` builder method. In
  both `Orchestrator::run` and `::stream`, every provider call is
  preceded by `pre_check(principal, estimated_usd, est_tokens)` and
  followed by `record(principal, estimated_usd, usage)`. Pre-flight
  cost uses `LlmProvider::estimate_cost_usd(&req)`; post-call cost
  reuses the same value (per-token rates aren't yet exposed on the
  trait). Pre-flight token estimate is `req.max_tokens.unwrap_or(0)`.
  `BudgetExhausted` errors short-circuit the run.
- Conductor / Trinity / SelfCaller budget wiring is intentionally
  deferred to v0.7.0 — same pattern, no public API surface disturbed.

- **Python facade for Phase-5 Rust additions**:
  - `tako.sigstore.KeylessVerifier(issuer, san, *, san_is_regex=False)`
    with `.verify_bundle(manifest, bundle)`. PyO3 binding
    `tako._native.KeylessVerifier`.
  - `tako.mcp.Grpc(endpoint, *, ca_pem=, ca_path=, client_cert_pem=,
    client_cert_path=, client_key_pem=, client_key_path=,
    domain_name=)` — accepts PEM either inline or from a filesystem
    path; the two are mutually exclusive.
  - `tako.budget.InMemoryBackend` joins `tako.budget.RedisBackend`
    with the same `current_usage` / `record` async API. Built into
    every wheel (no Cargo feature gate).
  - `tako.SingleAgent(provider, *, budget=, budget_backend=)` and
    `tako.Client(budget=, budget_backend=)` — kwargs flow through to
    the new Rust builder method.
- New PyO3 module pieces: `tako._native.InMemoryBudgetBackend`
  (always present); `tako._native.KeylessVerifier` (gated on
  `sigstore`); extended `tako._native.Grpc` constructor (gated on
  `grpc`); extended `tako._native.Orchestrator` constructor
  (`budget` / `budget_backend` kwargs).
- 12 new Python smoke tests:
  - `tests/python/test_phase5_sigstore_keyless.py` (4 cases) —
    auto-skip without `sigstore`. Generate the leaf cert via
    `cryptography` (already in the `dev` extra).
  - `tests/python/test_phase5_grpc_mtls.py` (3 cases) — auto-skip
    without `grpc`. Cover the validation rules; full mTLS round-trip
    coverage lives in the Rust integration tests.
  - `tests/python/test_phase5_budget_wiring.py` (5 cases) — always
    runs; `InMemoryBackend` round-trip, kwarg acceptance, pre-check
    short-circuit, recording usage, `Client` stashing.
- New examples: `examples/16_sigstore_keyless.py`,
  `examples/17_grpc_mtls.py`, `examples/18_budget_wired.py`.

### Notes

- Phase 5.C lands SingleAgent only. Conductor / Trinity / SelfCaller
  budget wiring is tracked for v0.7.0; the pattern is identical and
  the Python kwargs reuse the same `extract_budget_backend` helper.
- The keyless verifier's bundle JSON is intentionally simpler than
  cosign's protobuf bundle. A `--cosign-bundle` shim that converts the
  protobuf form to `KeylessBundle` is a candidate v0.7.0 ergonomics
  add.

## [0.5.0] - 2026-04-29

Phase 4 — Search & scale. Adds AB-MCTS orchestrator with verifiers
(landed pre-`[Unreleased]` against the previous tag) plus the Phase-4.D
through 4.G additions: a gRPC MCP transport, Sigstore tool-catalogue
verification, a Redis-backed `BudgetBackend`, and the matching PyO3 +
Python facade for all four. The previously-landed Phase-4.A AB-MCTS
orchestrator, Phase-4.B Mistral / Ollama providers, and Phase-4.C
WebSocket MCP transport are also published as part of this cut.

### Added

- **gRPC MCP transport** (`tako_mcp::GrpcTransport`, Phase 4.D): a fourth
  `McpTransport` impl alongside stdio, Streamable HTTP, and the Phase-4.C
  WebSocket transport. The `rmcp` crate ships no gRPC transport and the MCP
  spec doesn't standardise one, so we hand-craft a minimal JSON-RPC bridge:
  a single bidirectional streaming RPC (`tako.mcp.bridge.v1.McpBridge.Open`)
  carrying opaque `Frame { bytes json }` messages. Behaviour mirrors
  `WebSocketTransport`: a reader task spawned at `connect()` demuxes
  inbound frames into per-request `oneshot` channels (keyed by JSON-RPC
  `id`) and a `tokio::sync::broadcast` channel for server-emitted
  notifications; the outbound half is an `mpsc::Sender<Frame>` feeding
  `tonic`'s streaming request. `connect()` accepts both `http://` (plaintext)
  and `https://` (rustls + webpki-roots) endpoints; mTLS / custom CAs are
  out of scope and deferred to a later phase.
- Gated behind a new `grpc` Cargo feature on `tako-mcp` so `tonic` and the
  generated protobuf code only land in the dep tree when explicitly
  enabled. `protoc` is bundled via `protoc-bin-vendored` so contributors
  don't need a system-wide install to build with `--features grpc`; the
  `build.rs` no-ops entirely when the feature is off.
- Workspace `Cargo.toml` adds `tonic = "0.14"` (default-features off,
  `channel + codegen + router + transport + tls-ring + tls-webpki-roots`),
  `tonic-prost = "0.14"`, `tonic-prost-build = "0.14"`, `prost = "0.14"`,
  `tokio-stream = "0.1"`.
- Tests in `crates/tako-mcp/tests/grpc.rs` (4 cases, gated on `grpc`):
  happy-path JSON-RPC round-trip, 10 concurrent requests demuxed by id,
  broadcast notification fan-out, connect-error on a freed port. Server
  fixture is an in-process `tonic::transport::Server` bound to an
  ephemeral `127.0.0.1:0` port via `serve_with_incoming`.

- **Sigstore tool-catalogue verification** (`tako_governance::CatalogueVerifier`,
  Phase 4.E): an operator can pin the exact set of MCP tools a server is
  permitted to expose by signing a JSON catalogue with `cosign sign-blob`
  and shipping the catalogue + base64 signature alongside the server.
  `CatalogueVerifier::from_pem(cosign.pub)` loads the pinned key;
  `verifier.verify(manifest, signature) -> Catalogue` checks the cosign
  signature (raw or base64, ECDSA P-256 / Ed25519 / RSA) and returns the
  parsed `Catalogue { server, tools: Vec<ToolSchema> }`. The returned
  schemas pass straight to `tako_mcp::ToolRegistry::register_mcp` — no
  new coupling between `tako-governance` and `tako-mcp`.
- Trust model for this landing is **keyed** (pinned public key, the
  cosign default for `--key`); keyless verification (Fulcio cert + Rekor
  offline bundle against the Sigstore public-good trust root) is
  intentionally deferred — the same `verify` return shape will lift onto
  a bundle-based variant in a follow-up.
- Gated behind a new `sigstore` Cargo feature on `tako-governance` so
  the `sigstore` crate (and its `aws-lc-rs` crypto backend) only land in
  the dep tree when explicitly enabled.
- Workspace `Cargo.toml` adds `sigstore = "0.13"` with `default-features
  = false, features = ["cert"]` — the minimum for `CosignVerificationKey`
  + `SigStoreSigner`.
- Tests in `crates/tako-governance/tests/sigstore.rs` (6 cases, gated on
  `sigstore`): generates an ECDSA-P256 keypair at test time using
  `sigstore`'s own primitives so the fixtures are reproducible without
  `cosign` installed. Covers raw + base64 signature acceptance, tampered
  manifest detection, wrong-key rejection, malformed PEM rejection, and
  non-JSON payload rejection (after a valid signature).

- **Redis-backed `BudgetBackend`** (`tako_runtime::RedisBudgetBackend`,
  Phase 4.F): a multi-process `BudgetBackend` impl alongside the Phase-1
  `InMemoryBudgetBackend`. Keys are
  `<prefix>:{tenant_id}:{YYYY-MM-DD}` (UTC) so day rollover is automatic
  — tomorrow's writes land in a fresh key and yesterday's evicts via TTL
  (default 48 hours). `record()` is atomic via a small Lua script
  collapsing `HINCRBYFLOAT usd`, `HINCRBY tokens`, and `EXPIRE` into
  one round-trip. `current_usage()` is `HGETALL` (missing key → zero
  usage with no extra branching). `connect()` accepts both `redis://`
  (plaintext) and `rediss://` (TLS) URLs, and uses `redis::aio::ConnectionManager`
  for transparent reconnects on transient failures. `with_key_prefix`
  / `with_ttl` builder methods adjust the defaults.
- Gated behind a new `redis` Cargo feature on `tako-runtime` so the
  `redis` crate (and its TLS / async-runtime infrastructure) only land
  in the dep tree when explicitly enabled.
- Workspace `Cargo.toml` adds `redis = "1.2"` with `default-features =
  false, features = ["aio", "tokio-comp", "tokio-rustls-comp",
  "connection-manager", "script", "tls-rustls-webpki-roots"]` —
  matching the rustls + webpki-roots TLS choice used by `reqwest` and
  `tokio-tungstenite` elsewhere in the workspace. `chrono` is added as
  an optional dep on `tako-runtime` (gated by the same `redis` feature)
  for UTC day-key formatting.
- Tests in `crates/tako-runtime/tests/redis_budget.rs` (6 cases, gated
  on `redis` and auto-skipped when `REDIS_URL` is unset): missing-key
  zero-usage, record/read round-trip, multi-record accumulation,
  tenant isolation, daily-cap enforcement via `BudgetTracker`, and TTL
  application on the first record. Plus 2 unit tests in
  `src/budget_redis.rs` for the `format_day_key` pure function (date
  format stability + Unicode tenant IDs).

- **Python facade for Phase-4 Rust additions** (Phase 4.G): wires
  `WebSocketTransport`, `GrpcTransport`, `CatalogueVerifier`, and
  `RedisBudgetBackend` through to Pythonic surfaces.
  - `tako.mcp.WebSocket(url)` and `tako.mcp.Grpc(endpoint)` join the
    existing `Stdio` / `Http` transport classes; both run the
    `initialize` → `initialized` MCP handshake at construction time and
    plug into the orchestrator's heterogeneous `mcp_servers=[...]`
    arg via the extended
    `crates/tako-py/src/py_mcp.rs::extract_transport_handle`.
  - `tako.sigstore.CatalogueVerifier(pem)` (or
    `.from_pem_path(path)`) verifies a cosign-signed manifest and
    returns a `tako.sigstore.Catalogue` whose `.tools` are typed
    `tako.ToolSchema` objects ready to feed into a registry.
  - `tako.budget.RedisBackend(url, key_prefix=..., ttl_secs=...)`
    exposes the multi-process Redis budget backend with awaitable
    `current_usage(tenant_id) -> TenantUsage` and
    `record(tenant_id, usd, tokens) -> None` methods.
- New `tako-py` Cargo features: `ws`, `grpc`, `sigstore`, `redis` —
  each forwards to the matching feature on the underlying crate. The
  abi3 wheel is built with the desired subset, e.g.
  `maturin develop --features "ws grpc sigstore redis"`.
- New `crates/tako-py/src/{py_sigstore,py_runtime}.rs` modules;
  `py_mcp.rs` extended with `PyWebSocket` + `PyGrpc`.
- Python additions: new `python/tako/sigstore.py` module exporting
  `Catalogue` + `CatalogueVerifier`; `python/tako/budget.py` extended
  with `RedisBackend` + `TenantUsage`; `python/tako/mcp.py` extended
  with `WebSocket` + `Grpc`; `python/tako/_native.pyi` stubs updated.
- Tests in `tests/python/test_phase4_facades.py` (8 cases): each
  block auto-skips when its underlying class isn't on `_native` (so
  feature-stripped builds stay green). Sigstore tests use the
  `cryptography` Python library to generate an ECDSA-P256 keypair at
  test time and round-trip a signed manifest; Redis tests auto-skip
  when `REDIS_URL` is unset.
- `pyproject.toml` adds `cryptography>=43` to the `dev` extra (used
  only by the sigstore facade test; the runtime depends on neither).

### Notes

- The Python facade for `RedisBudgetBackend` exposes the backend as a
  standalone class with `record` / `current_usage`. Wiring it through
  `tako.Client` / `tako.SingleAgent` so the orchestrator
  automatically consults it is deferred — no current Python orchestrator
  surface accepts a `BudgetBackend` arg.

## [0.4.0] - 2026-04-29

Phase 3 — Learned coordination. Adds the Trinity router (rule-based +
ONNX), SelfCaller bounded-recursion wrapper, a Python training harness
+ eval harness, and replaces the Phase-2 streaming stubs in
`SingleAgent` and `Conductor` with native orchestrator-level streaming.

### Added

- **`Router` trait impls** in `tako-orchestrator`:
  - `RegexRouter`: rule-based default. Featurises the most-recent user
    message via the new shared `tako_orchestrator::features` module
    (16-dim `f32` vector) and routes through built-in code/math/fallback
    rules. `RegexRouter::builder()` accepts custom rule chains.
  - `OnnxRouter`: feature-gated behind the `onnx` Cargo feature
    (default off). Loads an ONNX classifier via `ort` 2.0.0-rc.10 with
    `load-dynamic` so the wheel stays slim. Featuriser parity with
    Python is asserted by `tests/python/test_features_parity.py`.

- **`Trinity` orchestrator** (`tako_orchestrator::Trinity`): per-turn
  role + model selection via a `Router`. Reuses the
  `HashMap<String, Arc<dyn LlmProvider>>` worker-pool shape from
  `Conductor` but with single-role-per-turn dispatch. PyO3 binding
  `tako._native.Trinity` + facade `tako.Trinity`.

- **`SelfCaller` orchestrator** (`tako_orchestrator::SelfCaller`):
  bounded-recursion wrapper over any `Arc<dyn Orchestrator>`. After
  each inner run, scores the output via `ConfidenceGuard::evaluate`;
  if below `min_confidence` AND depth `< max_depth`, recurses with a
  revision prompt appended. Depth tracked in
  `Principal.metadata["tako.recursion.depth"]` so accidental infinite
  loops are impossible.
  - `ConfidenceGuard` trait lives in `tako-core` alongside
    `AlwaysConfident` / `ConstantConfidence` test fixtures.
  - Guard impls in `tako-orchestrator`: `RuleBasedGuard` (regex +
    min-length) and `LlmJudgeGuard` (LLM-as-judge with parseable
    decimal output).
  - PyO3 bindings `tako._native.{SelfCaller, RuleBasedGuard,
    LlmJudgeGuard}` + Python facade `tako.SelfCaller` and
    `tako.guards.{RuleBased, LlmJudge}`.

- **Native orchestrator streaming** (carry-over from Phase 2.5):
  `SingleAgent::stream` and `Conductor::stream` now emit real
  `OrchEvent` streams instead of returning `Phase 2 stub` errors.
  `SingleAgent` forwards provider deltas as `OrchEvent::AssistantText`
  when the underlying provider's `supports_streaming` is true and
  falls back to `chat()` + one synthetic `AssistantText` otherwise.
  `Conductor` emits one `AssistantText` per coordinator turn plus
  `worker:<role>`-shaped `ToolCallStart` / `ToolCallResult` events for
  each dispatched worker. The `tako-compat` SSE emulation fallback is
  retained as a safety net for third-party orchestrators only.

- **Composable `Router` on `SingleAgent`**: new builder methods
  `.candidate(p)` and `.router(r)` enable per-step model selection over
  `[primary, ...candidates]` without role-switching. Backwards-compatible
  — without a router, the primary provider is used unconditionally.

- **Trinity training harness** (`python/tako/training/`):
  - `tako.training.features` — Python mirror of the Rust featuriser;
    parity asserted by a corpus test.
  - `tako.training.trinity.TrinityTrainer` — 2-layer MLP fit via numpy
    SGD. `fit_jsonl(path)` reads
    `{"prompt": ..., "label": ...}` rows; `export_onnx(path)` emits
    the model in the shape `OnnxRouter` consumes
    (`features:[1,16] → logits:[1,K]`).
  - CLI: `python -m tako.training.trinity --rollouts r.jsonl --out m.onnx`.
  - `numpy` and `onnx` are guarded by the new `tako[training]` extra so
    the base wheel stays slim.

- **Eval harness** (`python/tako/eval/`):
  - `Eval(orch, dataset, k=, concurrency=).run()` returns an
    `EvalReport` Pydantic model with pass-rate, p50/p95 latency, and
    per-task breakdowns. Phase-3 DoD requires "10-task synthetic
    benchmark + JSON report" — see
    `python/tako/eval/datasets/synthetic.jsonl` (math + factual + code
    mix).
  - `load_dataset("swe_bench_lite" | "gpqa_diamond")` raises
    `NotImplementedError` with explicit "Phase 4" pointers; no model
    weights or proprietary data committed.
  - CLI: `python -m tako.eval --orch module:fn --dataset synthetic --k 1 --out report.json`.

- **`tako._native.featurise_text(text)`** helper exposed for the
  parity test (Rust featuriser callable from Python).

- **Examples**: `13_trinity_router.py`, `14_self_caller.py`,
  `15_eval_harness.py`.

- **Docs**: new `concepts/routing.md`, `concepts/self_caller.md`,
  `recipes/trinity.md`, `recipes/self_caller.md`,
  `recipes/eval_harness.md`. `concepts/orchestrators.md` extended with
  Trinity + SelfCaller sections. mkdocs nav updated.

### Changed

- Workspace package version: `0.3.0` → `0.4.0` across `Cargo.toml`,
  `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py`.
- Workspace deps added: `ort` 2.0.0-rc.10 (default features off, `load-dynamic`
  + `ndarray`), `ndarray` 0.16. `tako-orchestrator` exposes them behind the
  `onnx` feature; `tako-py` forwards the feature.
- `tako-orchestrator` adds an `async-stream` 0.3 dep for the streaming
  generator helpers.
- `pyproject.toml` adds `[project.optional-dependencies] training = [...]`
  for the training harness's `numpy` + `onnx` deps.
- `Conductor::stream` extracts the worker-dispatch loop into a
  free-function `dispatch_workers_static` so both `run` and `stream`
  share one implementation.
- `tako._native.Orchestrator(...)` constructor adds optional
  `candidates=` and `router=` kwargs for the SingleAgent router opt-in.
- `tako._native.Trinity` accepts `roles` as a `list[tuple[str, Any]]`
  to preserve insertion order across the FFI boundary (HashMap iteration
  on the Rust side is otherwise nondeterministic).

### Deprecated

- (none)

### Removed

- `SingleAgent::stream` and `Conductor::stream` `"Phase 2"` error stubs
  — both now stream natively.

### Fixed

- `tako-orchestrator/src/single.rs` and `conductor.rs` model lookup
  now happens per-step (previously cached at the top of `run`),
  enabling per-step provider routing.

### Security

- (none)

## [0.3.0] - 2026-04-29

Phase 2.5 — cloud breadth + carry-overs. Adds Azure OpenAI and Vertex AI
(Gemini) providers; cloud secret resolvers for Vault, AWS Secrets
Manager, Azure Key Vault, and GCP Secret Manager; Bedrock streaming
(ConverseStream); OpenAI-compat SSE streaming; and a full mkdocs site
with GitHub Pages deploy.

### Added

- **Azure OpenAI provider** (`tako-providers-azure-openai`): same
  chat.completions wire format as OpenAI, but with the Azure URL shape
  (`/openai/deployments/{d}/chat/completions?api-version=...`) and
  `api-key` header auth. Provider id: `azure-openai:<deployment>`.
  PyO3 binding `tako._native.AzureOpenAi` + facade
  `tako.providers.AzureOpenAI`. 4 wiremock tests + 5 Python smoke tests.

- **Vertex AI provider** (`tako-providers-vertex`): Gemini via the
  `:generateContent` and `:streamGenerateContent?alt=sse` REST endpoints.
  Auth deferred to caller (pre-resolved OAuth2 access token via
  `.access_token()` / `.access_token_env()`); no `gcp_auth` dep added.
  Tool-call name correlation via id lookup against prior assistant
  messages. PyO3 binding `tako._native.Vertex` + facade
  `tako.providers.Vertex`. 5 wiremock tests + 5 Python smoke tests.

- **Cloud secret resolvers** in `tako-governance`:
  - `VaultResolver` (KV-v2 REST via reqwest; `path#field` JSON-pointer
    sub-key syntax).
  - `AwsSecretsManagerResolver` (`aws-sdk-secretsmanager`; deferred
    credential chain resolution; `name#version` syntax).
  - `AzureKeyVaultResolver` (REST via reqwest; deferred bearer token;
    `name#version` syntax).
  - `GcpSecretManagerResolver` (REST via reqwest; deferred bearer
    token; `name#version` syntax; base64-decodes payload).
  PyO3 bindings `tako._native.{Vault,AzureKeyVault,GcpSecretManager,
  AwsSecretsManager}Resolver` + new facade module `tako.secrets`.
  Refactor: `secrets.rs` -> `secrets/` module (mod.rs + 4 impl files).
  10 wiremock-backed Rust tests + 7 Python smoke tests.

- **Bedrock streaming**: replaces v0.2.0's `Phase 2.5` 501 stub with a
  real `ConverseStream` implementation. `stream::map_event` walks each
  event variant (MessageStart, ContentBlockStart::ToolUse,
  ContentBlockDelta::Text/ToolUse, MessageStop, Metadata) and emits
  `ChatChunk::Delta` / `End` / `Error`. Capabilities flag
  `supports_streaming` flips to `true`. 5 unit tests covering each
  branch.

- **tako-compat SSE streaming**: replaces v0.2.0's `stream=true` 501
  with a real `axum::response::sse::Sse` stream. `sse::event_to_payloads`
  reverse-maps `OrchEvent` -> OpenAI `chat.completion.chunk` JSON +
  terminal `data: [DONE]` line, matching what the official `openai`
  Python SDK consumes. When the underlying orchestrator's `stream()`
  isn't implemented, falls back to running `run()` and emulating one
  AssistantText chunk + Final — wire format is identical either way.
  4 sse unit tests + replaces the obsolete `returns_501` server
  integration test with one that asserts SSE chunks + DONE.
  `tests/python/test_compat_streaming.py` includes both a raw-SSE
  wire-format test and an `openai` SDK conformance test (skip-if-not-
  installed).

- **mkdocs site**: full nav under `docs/`:
  - `concepts/`: providers, orchestrators, policy, secrets, budgets,
    tracing, mcp.
  - `recipes/`: azure_openai, vertex, bedrock, openai_compat_server,
    conductor, opa_policy, secret_resolvers.
  - `api/`: python (mkdocstrings), rust (docs.rs links).
  Material theme with light+dark, navigation.sections, search.highlight.
  `mkdocs.yml` moves to repo root (modern Material requirement).
  `mkdocs build --strict` is clean.

- **`.github/workflows/docs.yml`**: builds the mkdocs site on push to
  main when `docs/` or `python/tako/` change, deploys to GitHub Pages
  via `actions/deploy-pages@v4`. Repo Pages source must be set to
  'GitHub Actions' once post-merge.

- Examples: `09_azure_openai.py`, `10_vertex_gemini.py`,
  `11_secrets_vault.py`, `12_bedrock_streaming.py`.

### Changed

- Workspace package version: `0.2.0` -> `0.3.0` across `Cargo.toml`,
  `pyproject.toml`, `python/tako/__init__.py`, `tests/python/test_smoke.py`.
- `Bedrock` `supports_streaming` capability flips to `true`.
- `tako-providers-openai` exposes `convert` and `stream` modules as
  `#[doc(hidden)] pub mod` so the Azure OpenAI crate can reuse them.
- Workspace deps added: `aws-sdk-secretsmanager` 1.83, `base64` 0.22.
  Bedrock crate adds `async-stream` 0.3 (already a dep of openai/anthropic
  providers).
- `tako-governance/Cargo.toml` adds `reqwest`, `base64`, `aws-config`,
  `aws-sdk-secretsmanager` for cloud resolvers; `wiremock` as dev-dep.

### Deprecated

- (none)

### Removed

- The `Phase 2.5` 501 stubs in `BedrockProvider::stream()` and
  `tako-compat`'s `chat_completions` for `stream=true`. Both replaced
  with real streaming.

### Fixed

- Bedrock provider's `supports_streaming` capability incorrectly read
  `false`; flipped to `true` now that streaming works.

### Security

- (none net new — cloud resolvers all use the same SecretString
  redaction story as `EnvResolver`)

## [0.2.0] - 2026-04-29

Phase 2 + bundled Phase 1.5 follow-ups. Adds Conductor, Bedrock,
OPA/Rego enforcement, an OpenAI-compatible HTTP server, and closes the
remaining Python-parity gaps from Phase 1 (MCP transports,
`PythonProvider`, OTLP exporter).

### Added

- **Phase 1.5 — Python parity:**
  - `tako._native.Stdio(command, args)` and `tako._native.StreamableHttp(url, ...)`
    plus `tako.mcp.Stdio` / `tako.mcp.Http` Python wrappers.
  - `tako.SingleAgent(provider, mcp_servers=[...])` discovers tools at
    construction time via MCP `tools/list`.
  - `tako._native.PythonProvider(id, chat=...)` + `tako.providers.PythonProvider`:
    user-defined `LlmProvider`s in pure Python via an async callable.
    GIL-correct hand-off (`Python::attach` → `into_future` →
    await-without-GIL).
  - Real OTLP gRPC exporter via `opentelemetry-otlp` 0.31 + tonic.
    `tako.tracing.init_otlp(endpoint, ...)` + `shutdown_otlp()`. Process-
    global guard flushes pending spans on interpreter exit.

- **Phase 2 features:**
  - `tako-providers/bedrock`: Amazon Bedrock provider via the Converse
    API (`aws-sdk-bedrockruntime` 1.130). Supports text, tool calls, and
    tool results; system messages hoist to the top-level `system` field.
    Streaming (ConverseStream) is documented as Phase 2.5.
    `tako._native.Bedrock` + `tako.providers.Bedrock` Python wrappers.
  - `Conductor` orchestrator (arXiv:2512.04388 generalisation): a
    coordinator LLM emits structured dispatch JSON; workers keyed by role
    name (`code`, `math`, …) run concurrently under an `Arc<Semaphore>`
    capped at `max_fanout`. Configurable `max_steps`, `worker_timeout`,
    `fail_fast`. Markdown ` ```json ` fences are stripped; malformed
    output is fed back as a one-turn retry. `tako.Conductor(...)` Python
    wrapper.
  - `tako_governance::policy`: OPA/Rego enforcement via `regorus` 0.9.
    `OpaBundle::from_string` / `from_path` with SHA-256 source caching;
    `PolicyEngine` impl for three stages (`PreChat`, `PreTool`,
    `PostChat`). `AuditLog::jsonl(path)` + `in_memory()` writes
    every decision as JSONL. `SingleAgentBuilder::policy(...)` consults
    the engine before each tool invocation; `Deny` /
    `RequireApproval` propagate as `TakoError::PolicyDenied`.
  - `tako-compat`: OpenAI-compatible HTTP server (`axum` 0.8). Routes:
    `POST /v1/chat/completions` (non-streaming), `GET /v1/models`,
    `GET /healthz`, `GET /readyz`. Bearer-token auth via `AuthResolver`
    + `StaticTokens`. `tako._native.serve_openai_py` +
    `tako.compat.serve_openai(orch, host, port, tokens, models)`.
    Streaming SSE deferred to Phase 2.5; stream requests return 501.

- Examples: `02_conductor.py`, `07_openai_compat_server.py`.
- Python tests: `test_mcp_stdio.py`, `test_python_provider.py`,
  `test_otlp.py`, `test_conductor.py`, `test_compat_server.py`
  (now 20 Python tests; was 8 in Phase 1).
- Rust tests: 7 conductor cases, 2 policy E2E cases, 6 bedrock convert
  cases, 6 compat-server cases, 4 OPA-policy unit cases (~94 total).

### Changed

- Workspace package version: `0.1.0` → `0.2.0` across `Cargo.toml`,
  `pyproject.toml`, `python/tako/__init__.py`.
- Workspace deps added: `regorus` 0.9, `aws-config` 1.8,
  `aws-sdk-bedrockruntime` 1.130, `axum` 0.8, `tower` 0.5,
  `tower-http` 0.6, `hyper` 1.
- New workspace member: `crates/tako-compat`,
  `crates/tako-providers/bedrock`.

### Deprecated

- (none)

### Removed

- The Phase-1 placeholder `tako.tracing.Otlp` no-op was replaced with a
  config object that delegates to `init_otlp`.

### Fixed

- `tako_governance::otel::init_otlp_tracing` now actually wires an OTLP
  exporter (was a warn-and-delegate stub in Phase 1). Constructor enters
  the shared Tokio runtime handle so hyper-util doesn't panic on the
  missing reactor.

### Security

- All policy decisions through `OpaBundle` are recorded to the configured
  `AuditLog` for SIEM ingestion (JSONL: timestamp, principal, stage,
  decision, model).

## [0.1.0] - 2026-04-28

Initial Phase 1 foundation release.

### Added

- Initial workspace scaffolding for the Phase 1 foundation:
  `tako-core`, `tako-runtime`, `tako-providers/{anthropic,openai,http-generic}`,
  `tako-mcp`, `tako-orchestrator`, `tako-governance`, `tako-py`.
- Five core async traits in `tako-core`: `LlmProvider`, `Tool`, `McpTransport`,
  `Router`, `PolicyEngine`.
- `SingleAgent` orchestrator with a max-step tool-call loop.
- Anthropic Messages and OpenAI Chat Completions providers with streaming SSE
  and tool calls.
- MCP client transports: stdio (subprocess) and Streamable HTTP, via `rmcp`.
- In-memory budget tracker with a pluggable `BudgetBackend` trait.
- `failsafe`-backed circuit breaker, `governor` rate limiter, retry-with-jitter.
- OpenTelemetry pipeline emitting `tako.*` and `gen_ai.*` semconv attributes
  (stub OTLP exporter; real wiring landed in 0.2.0).
- Presidio-style PII regex content transform (mask / hash / redact).
- PyO3 bindings (`tako._native`) plus a Pydantic-v2 Python facade
  (`python/tako/`).
- Sync + async dual API: every async method has a `_sync` sibling.
- CI workflows: fmt + clippy + cargo test + maturin develop + pytest +
  cargo-audit + pip-audit on Linux/macOS/Windows.

### Changed

- Pinned crate versions to current stable as of 2026-04-28; differs from the
  spec snapshot:
  - `tokio` 1.43 → 1.52, `reqwest` 0.12 → 0.13, `governor` 0.7 → 0.10,
    `schemars` 0.8 → 1.2, `rmcp` 0.16 → 1.5, `regorus` 0.4 → 0.9,
    `sigstore` 0.10 → 0.13, `tokio-tungstenite` 0.24 → 0.29,
    `tonic` 0.12 → 0.14, `prost` 0.13 → 0.14, `ort` rc.10 → rc.12,
    `aws-sdk-bedrockruntime` 1.50 → 1.130.

### Security

- `cargo audit` and `pip-audit` integrated into CI.

[Unreleased]: https://github.com/nyankobu010/tako-ai-core/compare/v0.42.0...HEAD
[0.42.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.41.0...v0.42.0
[0.41.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.40.0...v0.41.0
[0.40.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.39.0...v0.40.0
[0.39.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.38.0...v0.39.0
[0.38.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.37.0...v0.38.0
[0.37.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.36.0...v0.37.0
[0.36.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.35.0...v0.36.0
[0.35.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.34.0...v0.35.0
[0.34.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.33.0...v0.34.0
[0.33.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.32.0...v0.33.0
[0.32.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.31.0...v0.32.0
[0.31.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.30.0...v0.31.0
[0.30.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.29.0...v0.30.0
[0.29.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.28.0...v0.29.0
[0.28.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.27.0...v0.28.0
[0.27.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.26.0...v0.27.0
[0.26.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.25.0...v0.26.0
[0.25.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.24.0...v0.25.0
[0.24.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.23.0...v0.24.0
[0.23.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.22.0...v0.23.0
[0.22.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.21.0...v0.22.0
[0.21.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.20.0...v0.21.0
[0.20.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.18.0...v0.19.0
[0.18.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/nyankobu010/tako-ai-core/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/nyankobu010/tako-ai-core/releases/tag/v0.1.0
