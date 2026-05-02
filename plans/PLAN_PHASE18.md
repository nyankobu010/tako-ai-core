# PLAN — Phase 18 (OIDC introspection asymmetric JWT + end-session helper)

## Context

Phase 17 (v0.18.0, 2026-05-01) added discovery-driven OIDC
introspection auth-method selection (RFC 8414) and the symmetric
`client_secret_jwt` introspection auth method (RFC 7521 / 7523,
HS256 over `client_secret`). [`PLAN.md`](PLAN.md) lines 49–66 list
the Phase-18 carry-forward.

Phase 18 closes two more items, both clean extensions of the
17.A / 17.B work:

1. **Asymmetric `private_key_jwt` introspection auth method.**
   The natural sibling of 17.B's `client_secret_jwt`. Uses an
   RSA / EC private key (RS256 / ES256) instead of the symmetric
   `client_secret` to sign the same RFC 7521 / 7523 client
   assertion. Closes the second of the two RFC 7521-defined JWT
   client-auth flavours.
2. **OIDC end-session endpoint helper.** The OIDC Session
   Management 1.0 spec defines `end_session_endpoint` as a
   discovery-doc field and a query-string-formatted URI for
   server-initiated logout. tako captured `introspection_endpoint`
   in 15.B.2; this phase captures `end_session_endpoint` the same
   way and provides a small URL-builder helper.

mTLS introspection auth methods (`tls_client_auth` /
`self_signed_tls_client_auth`) remain deferred to Phase 19+ —
they need workspace-level reqwest TLS feature changes (client
identity material plumbed through `reqwest::ClientBuilder`)
that warrant a focused phase. Refresh-token flows are a different
model entirely (tako as token *consumer* rather than validator)
and need a new component, also deferred.

All three sub-items are strictly additive — public APIs unchanged
shape.

**Theme:** *Continue OIDC introspection completeness from 17.B;
add a small SSO-logout helper.*

**Tag:** v0.19.0.

## A. `private_key_jwt` introspection auth method

### A.1 — Storage for the asymmetric signing key

`EncodingKey` from `jsonwebtoken` does not impl `Clone`. To stay
compatible with `OidcAuthResolver`'s existing `#[derive(Clone)]`
(used by the Python facade's immutable-builder pattern in
[`crates/tako-py/src/py_compat.rs`](/Users/kwc/tako-ai-core/crates/tako-py/src/py_compat.rs)),
the new key + algorithm fields land behind `Arc`:

```rust
pub struct ClientAssertionKey {
    pub algorithm: Algorithm,
    encoding_key: EncodingKey,
}

pub struct IntrospectionConfig {
    // ... existing fields
    /// Phase 18.A — asymmetric signing key for
    /// `IntrospectionAuthMethod::PrivateKeyJwt`. None for
    /// symmetric methods (Basic / Post / `client_secret_jwt`).
    pub client_assertion_key: Option<Arc<ClientAssertionKey>>,
}
```

`ClientAssertionKey` ships three constructors:
- `from_rs256_pem(&[u8])` — RS256 (industry default)
- `from_es256_pem(&[u8])` — ES256
- `from_ed25519_pem(&[u8])` — EdDSA (newer; `jsonwebtoken` ≥ 9.x
  supports it)

A raw `from_encoding_key(EncodingKey, Algorithm)` constructor stays
out of scope — keep the public surface minimal and explicit so
callers can't accidentally pass an HS-secret as RSA.

`Debug` is implemented manually to redact the key body; the
algorithm is fine to log.

### A.2 — Enum extension

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IntrospectionAuthMethod {
    #[default]
    ClientSecretBasic,
    ClientSecretPost,
    ClientSecretJwt,
    /// Phase 18.A — RFC 7521 / 7523 asymmetric client-assertion
    /// JWT auth. Signs the same claim layout as `ClientSecretJwt`
    /// but with the RSA / EC private key from
    /// `IntrospectionConfig::client_assertion_key`. Errors at
    /// request time when no key is configured.
    PrivateKeyJwt,
}
```

The enum stays unit-variant so all the
`#[derive(Copy, PartialEq, Eq)]` machinery from 17.B keeps working.

### A.3 — Builders + auto-selector extension

Two new `OidcAuthResolver` chainable builders:

- `with_introspection_private_key(key: ClientAssertionKey)` —
  attaches an asymmetric signing key to the existing
  introspection config. Silent no-op when no introspection has
  been configured yet (matches the 16.B.2 / 17.A
  chainable-builder cadence).
- `with_introspection_jwt_rs256_pem(pem: &[u8])` /
  `with_introspection_jwt_es256_pem(pem: &[u8])` — convenience
  combos that call `ClientAssertionKey::from_*_pem` then
  `with_introspection_private_key`. The error path returns
  `TakoError::Invalid` on PEM parse failure.

Phase 17.A's auto-selector (line 364-389 in
[`oidc.rs`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs))
extends to a four-tier preference order:

1. `private_key_jwt` (only when an asymmetric key is configured)
2. `client_secret_jwt` (only when a `client_secret` is configured)
3. `client_secret_basic`
4. `client_secret_post`

The fail-closed branch fires when discovery advertised a list
with **no** supported variant given the configured credentials —
e.g. issuer requires only `tls_client_auth` (mTLS deferred to
Phase 19+).

### A.4 — `introspect()` branch

Refactor the existing `build_client_assertion_hs256` helper
(line 670-705 in
[`oidc.rs`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs))
into a single `build_client_assertion` that accepts
`&EncodingKey` + `Algorithm` directly. The `ClientSecretJwt`
path passes `EncodingKey::from_secret(client_secret.as_bytes())`
+ `Algorithm::HS256`; the new `PrivateKeyJwt` path passes the
borrowed key from `cfg.client_assertion_key`.

Wire shape: identical to `client_secret_jwt`. Form body fields:
- `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`
- `client_assertion=<jwt>`

No `Authorization` header. Per RFC 7521 §4.2 / 7523 §3 the
`aud` claim binds the assertion to the introspection endpoint.

Errors at request time when `PrivateKeyJwt` is selected but
`cfg.client_assertion_key.is_none()`.

### A.5 — Tests

Six new tests in
[`crates/tako-compat/src/auth/oidc.rs`](/Users/kwc/tako-ai-core/crates/tako-compat/src/auth/oidc.rs):

1. `client_assertion_key_from_rs256_pem_round_trip` — accepts a
   valid RSA private-key PEM, rejects garbage with
   `TakoError::Invalid`.
2. `auto_select_prefers_private_key_jwt_when_listed_and_key_present` —
   discovery advertises the full set; auto-selector picks
   `PrivateKeyJwt`.
3. `auto_select_skips_private_key_jwt_when_no_key` — even when
   listed, falls back to the next supported variant.
4. `introspect_private_key_jwt_errors_when_key_missing` —
   request-time fail on missing key.
5. `introspect_private_key_jwt_signed_with_rs256` — wiremock test
   that captures the posted body, parses out the `client_assertion`
   JWT, verifies the RS256 signature against the matching public
   key, and asserts the claim layout (`iss` / `sub` = `client_id`,
   `aud` = `introspect_uri`, `exp` ~ 30s in the future).
6. `introspect_private_key_jwt_carries_client_assertion_form_fields` —
   shape assertion: form body has the type URI + assertion JWT,
   no `Authorization: Basic`, no `client_secret=`.

## B. OIDC end-session endpoint helper

### B.1 — Discovery doc capture

Extend `DiscoveryDoc` with `end_session_endpoint:
Option<String>` (`#[serde(default)]`). Thread the value into a
new `OidcAuthResolver::discovered_end_session_uri:
Option<String>` field.

### B.2 — Public accessor + URL builder

Two new public methods on `OidcAuthResolver`:

```rust
/// Phase 18.B — return the issuer's `end_session_endpoint` URL
/// captured at discovery time. `None` when the issuer doesn't
/// advertise OIDC Session Management.
pub fn end_session_endpoint(&self) -> Option<&str> { ... }

/// Phase 18.B — build a logout URL per OIDC Session Management
/// 1.0 §5. Returns `None` when the issuer didn't advertise
/// `end_session_endpoint`. All query params are optional;
/// passing `None` for everything yields just the bare endpoint
/// URL.
///
/// Spec params honoured:
/// - `id_token_hint` — the access token / ID token the
///   end-user wants to terminate (recommended)
/// - `post_logout_redirect_uri` — where to send the user-agent
///   after logout completes
/// - `state` — CSRF mitigation; round-tripped on the redirect
pub fn build_logout_uri(
    &self,
    id_token_hint: Option<&str>,
    post_logout_redirect_uri: Option<&str>,
    state: Option<&str>,
) -> Option<String> { ... }
```

Implementation uses the same `url::form_urlencoded::Serializer`
pattern that `introspect()` uses — the serializer is built and
dropped synchronously, so no `Send` issues.

Pure read of a discovery field + URL builder, no async, no I/O.

### B.3 — Tests

Three new tests:

1. `discovery_doc_parses_optional_end_session_endpoint` — round
   trips with-and-without the field.
2. `end_session_endpoint_accessor_returns_captured_uri` — verify
   the field is plumbed.
3. `build_logout_uri_with_all_params` — assert the formatted URI
   contains `id_token_hint`, `post_logout_redirect_uri` (URL-
   encoded), and `state`. `None` parameters are omitted.
4. `build_logout_uri_returns_none_when_not_advertised` — when
   the issuer didn't advertise.

## C. Python facade mirror

[`crates/tako-py/src/py_compat.rs`](/Users/kwc/tako-ai-core/crates/tako-py/src/py_compat.rs):

- `OidcAuth.with_introspection_jwt_rs256_pem(pem: bytes)` — load an
  RSA private-key PEM and switch the introspection auth method to
  `PrivateKeyJwt`. Returns a fresh `OidcAuth`.
- `OidcAuth.with_introspection_jwt_es256_pem(pem: bytes)` — ES256
  sibling.
- `OidcAuth.with_introspection_auth_method(method)` — alias parser
  extended to accept case-insensitive `"private_key_jwt"` /
  `"private-key-jwt"`. (The existing `with_introspection_*_pem`
  builders set the auth method as a side-effect; this alias lets
  the auth method itself be reset independently.)
- `OidcAuth.end_session_endpoint() -> Optional[str]` — pyo3
  reflects the captured discovery field.
- `OidcAuth.build_logout_uri(id_token_hint=None, post_logout_redirect_uri=None, state=None) -> Optional[str]` —
  the URL builder.

[`python/tako/compat.py`](python/tako/compat.py) module docstring
updated to mention the new entry points.
[`tests/python/test_phase18_oidc.py`](tests/python/test_phase18_oidc.py)
covers facade attribute presence; the eight new Rust tests across
18.A + 18.B remain the source of truth for behaviour.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  `private_key_jwt` tests in 18.A.5 pass; the new end-session
  tests in 18.B.3 pass; existing 17.A / 17.B tests still
  byte-for-byte green.
- `pytest -q tests/python/test_phase18_oidc.py` — green on a
  wheel built with `--features auth-oidc`.

## Out of scope (Phase 19+)

- mTLS (`tls_client_auth` / `self_signed_tls_client_auth`)
  introspection auth methods — needs reqwest TLS feature changes
  at workspace scope.
- OIDC refresh-token flows (`refresh_token` grant, `revocation_endpoint`
  / RFC 7009).
- OIDC client-credentials grant (server-to-server token issuance).
- Composite `AuthResolver`s (mTLS + bearer chaining).
- Vision / image content support across Anthropic / Vertex /
  Bedrock — warrants a dedicated phase, cross-cutting across
  three provider crates.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond) —
  needs a sandboxed runner.
- OTel end-to-end real-collector test.

## Commits

1. `feat(tako-compat): private_key_jwt OIDC introspection auth method (Phase 18.A)`
2. `feat(tako-compat): OIDC end-session endpoint helper (Phase 18.B)`
3. `feat(tako-py): OIDC private_key_jwt + end-session facade (Phase 18.C)`
4. `docs: Phase 18 PLAN/README/CHANGELOG flip (v0.19.0)`
