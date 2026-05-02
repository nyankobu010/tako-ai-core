# PLAN — Phase 17 (OIDC introspection completeness)

## Context

Phase 16 (v0.17.0, 2026-05-01) closed three carry-forward items from
the Phase 13–15 holding pen: bounded mpsc backpressure on the
streaming-verifier rollouts in [`AbMcts::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/ab_mcts.rs)
and [`Conductor::stream`](/Users/kwc/tako-ai-core/crates/tako-orchestrator/src/conductor.rs);
Vault Enterprise namespace support on
[`VaultAuthResolver`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/vault.rs);
and the RFC 7662 §2.1 `client_secret_post` introspection auth method
on [`OidcAuthResolver`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs).

[`PLAN.md`](PLAN.md) lines 56–65 enumerate the Phase 17 candidates.
The cleanest follow-up — and a direct continuation of the 16.B.2
work — is to finish the OIDC introspection auth-method surface that
RFC 7662 §2.1 enumerates:

1. **Discovery-driven auth-method selection** (RFC 8414 — read
   `introspection_endpoint_auth_methods_supported` from the discovery
   doc and auto-select rather than the operator opting in by builder).
2. **`client_secret_jwt`** — RFC 7521 / 7523 client-assertion JWT
   signed with HS256 over the client_secret, sent as
   `client_assertion` + `client_assertion_type` form fields.

mTLS (`tls_client_auth` / `self_signed_tls_client_auth`) needs client
TLS material plumbed through `reqwest::ClientBuilder` and stays
deferred to Phase 18+. Likewise `private_key_jwt` (asymmetric JWT
client auth — needs separate signing-key storage) defers — Phase 17
restricts to HS256-over-`client_secret` because no new key material
is needed.

All three sub-items are strictly additive — public APIs unchanged
shape.

**Theme:** *Finish the OIDC introspection auth-method surface
shipped in 15.B.2 + 16.B.2.*

**Tag:** v0.18.0.

## A. Discovery-driven introspection auth-method selection

### A.1 — Capture supported list from discovery

[`crates/tako-compat/src/auth/oidc.rs`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs):
extend `DiscoveryDoc` with an
`introspection_endpoint_auth_methods_supported: Option<Vec<String>>`
field (`#[serde(default)]`); thread the value into a new
`OidcAuthResolver::discovered_introspection_auth_methods:
Option<Vec<String>>` field captured at construction.

A missing field is recorded as `None`, *not* `Some(vec![])` — the
two states are semantically distinct (RFC 8414 says the default when
absent is `client_secret_basic`).

### A.2 — `OidcAuthResolver::with_introspection_auth_method_from_discovery`

New chainable builder. Behaviour:

- If `self.introspection.is_none()`: silent no-op (matches the
  16.B.2 cadence for `with_introspection_auth_method` —
  Phase-16.B.2 design choice).
- If discovery supplied no list (`None`): pick `ClientSecretBasic`
  (RFC 8414 default).
- If discovery supplied a list: pick the strongest supported
  variant in this preference order:
  1. `client_secret_jwt` *if Phase 17.B is also configured* (i.e.
     `client_secret` is non-`None` — see B.3 below for the
     "credentials present" precondition).
  2. `client_secret_basic`.
  3. `client_secret_post`.
- If none of the discovery list matches a supported variant:
  return `Err(TakoError::Invalid("oidc: no supported
  introspection auth method advertised by issuer; supported:
  [...]"))`. (Fail-closed; otherwise the caller silently keeps
  the default Basic and may surprise the operator.)

Why fail-closed only on the "discovery-listed-but-none-supported"
case: if we silently fell back to Basic when discovery
*explicitly* listed only `tls_client_auth` and `private_key_jwt`,
the resolver would send credentials the issuer is configured to
reject — better to surface that at builder time than at
introspection-time HTTP-401.

Five new tests:
- discovery-doc with `["client_secret_basic"]` → picks Basic.
- discovery-doc with `["client_secret_post"]` → picks Post.
- discovery-doc with `["client_secret_jwt", "client_secret_basic"]`
  + `client_secret = Some` → picks Jwt.
- discovery-doc with `["client_secret_jwt"]` + `client_secret =
  None` → picks nothing supported → Err.
- discovery-doc field absent → picks Basic.

## B. `client_secret_jwt` introspection auth method (RFC 7523)

### B.1 — Enum extension + config

[`crates/tako-compat/src/auth/oidc.rs`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs):
extend `IntrospectionAuthMethod` with a third unit variant
`ClientSecretJwt`. The enum stays `#[derive(Debug, Clone, Copy,
Default, PartialEq, Eq)]` — no field-bearing variants, so all the
Copy/Eq machinery that 16.B.2 set up keeps working.

Phase 17 restricts to HS256-over-`client_secret` — no new key
material needed. The asymmetric `private_key_jwt` flavour (RS256 /
ES256 with a separate signing key) is deferred — would need a new
`introspection_signing_key: Option<EncodingKey>` field (and
`EncodingKey` doesn't impl `Clone` cleanly).

### B.2 — `introspect()` branch

When `cfg.auth_method == ClientSecretJwt`:

1. Require `cfg.client_secret.is_some()` — else
   `TakoError::Invalid("oidc: client_secret_jwt requires
   client_secret to be set")` (HS256 needs the symmetric key).
2. Build a JWT with header `{ "alg": "HS256", "typ": "JWT" }` and
   claims:
   - `iss` = `cfg.client_id`
   - `sub` = `cfg.client_id`
   - `aud` = `cfg.introspect_uri`
   - `iat` = `now_unix_seconds()`
   - `exp` = `iat + 30` (RFC 7521 §4.2 recommends "short
     lifetime"; 30s matches industry practice)
   - `jti` = a per-request UUID (use existing `uuid` workspace
     dep if present, else inline a 16-byte random hex from
     `rand::random::<[u8; 16]>()`).
3. Sign with `jsonwebtoken::encode` using
   `EncodingKey::from_secret(client_secret.as_bytes())` and
   `Header::new(Algorithm::HS256)`.
4. Send the form body with three fields:
   `token=<jwt>&client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer&client_assertion=<assertion-jwt>`.
   No Authorization header (per RFC 7521 §4.2).

The JWT-build step uses `jsonwebtoken::encode` which is sync and
allocates a `String` — no `.await` to break the
`url::form_urlencoded::Serializer`-not-Send tight scope. Build the
form body in the same scope, drop both, then await the POST.

### B.3 — Credentials-present precondition

`with_introspection_auth_method(ClientSecretJwt)` accepts any
`client_secret` state at builder time (preserves the chainable
builder pattern), but `introspect()` errors at request time when
the secret is absent. This matches the existing pattern (the same
introspect call already errors on transport / 5xx / `active=false`
at request time, not builder time).

Phase 17.A's auto-selector skips `ClientSecretJwt` when
`client_secret.is_none()` so the discovery-driven path can't pick
an unusable method.

### B.4 — Tests

Three new wiremock-based tests in
[`crates/tako-compat/src/auth/oidc.rs`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs):

1. `introspect_jwt_carries_client_assertion_form_fields`: assert the
   POST body contains `client_assertion_type=urn%3Aietf%3Aparams%3Aoauth%3Aclient-assertion-type%3Ajwt-bearer`
   and `client_assertion=<jwt>`. No `Authorization: Basic` header,
   no `client_secret=` field.
2. `introspect_jwt_signed_with_client_secret_hs256`: capture the
   posted body, parse out the `client_assertion` JWT, verify the
   signature against the configured `client_secret` using
   `jsonwebtoken::decode`, assert claims (`iss`/`sub`=`client_id`,
   `aud`=`introspect_uri`, `exp` in the near future).
3. `introspect_jwt_errors_when_secret_missing`: configure
   `with_introspection_uri(uri, "client", None)` and
   `with_introspection_auth_method(ClientSecretJwt)`, call
   `introspect("any")`, assert it returns
   `TakoError::Invalid("oidc: client_secret_jwt requires...")`.

## C. Python facade mirror

[`crates/tako-py/src/py_compat.rs`](/Users/kwc/tako-ai-core/crates/tako-py/src/py_compat.rs):

- `tako.compat.OidcAuth.with_introspection_auth_method(method)` —
  extend the alias parser to accept case-insensitive `"jwt"` /
  `"client_secret_jwt"` (in addition to the existing four aliases
  for Basic / Post). Maps to
  `IntrospectionAuthMethod::ClientSecretJwt`.
- `tako.compat.OidcAuth.with_introspection_auth_method_from_discovery()` —
  new chainable instance method. Returns a fresh `OidcAuth`;
  raises `ValueError` on the fail-closed case (none of the
  discovery list matches a supported variant); silent on the
  no-introspection-yet case (matches 16.B.2 cadence).

Update [`python/tako/compat.py`](/Users/kwc/tako-ai-core/python/tako/compat.py)
docstring to mention the two new entry points.

[`tests/python/test_phase17_oidc.py`](/Users/kwc/tako-ai-core/tests/python/test_phase17_oidc.py)
covers the facade attribute presence and the alias-parsing edge
cases (`"jwt"` accepted; `"jwt-bearer"` rejected; case-insensitive).
Rust tests remain the source of truth for behaviour.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  discovery-driven selection tests in 17.A.2 pass; the new
  `client_secret_jwt` tests in 17.B.4 pass; existing 15.B.2 / 16.B.2
  Basic/Post wire tests still byte-for-byte green.
- `pytest -q tests/python/test_phase17_oidc.py` — green on a wheel
  built with `--features auth-oidc`.

## Out of scope (Phase 18+)

- mTLS (`tls_client_auth` / `self_signed_tls_client_auth`)
  introspection auth methods — needs client TLS material plumbed
  through `reqwest::ClientBuilder`.
- `private_key_jwt` (asymmetric JWT client auth — RS256 / ES256
  with separate signing-key storage).
- OIDC refresh-token flows / `end_session_endpoint` helper.
- Composite `AuthResolver`s (mTLS + bearer chaining).
- Vision / image content support across Anthropic / Vertex /
  Bedrock — warrants a dedicated phase, cross-cutting across three
  provider crates.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond) —
  needs a sandboxed runner.
- OTel end-to-end real-collector test.

## Commits

1. `feat(tako-compat): discovery-driven OIDC introspection auth-method selection (Phase 17.A)`
2. `feat(tako-compat): client_secret_jwt OIDC introspection auth method (Phase 17.B)`
3. `feat(tako-py): OIDC introspection JWT + auto-select facade (Phase 17.C)`
4. `docs: Phase 17 PLAN/README/CHANGELOG flip (v0.18.0)`
