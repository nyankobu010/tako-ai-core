# PLAN — Phase 25 (OIDC `self_signed_tls_client_auth`)

## Context

Phase 24 (v0.25.0, 2026-05-01) closed the OIDC introspection mTLS
gap with the `tls_client_auth` variant (RFC 8705 §2.1). After Phase
24 the auth-method surface covers five of the six methods listed
in RFC 7662 §2.1 / RFC 8414 / RFC 8705. The sixth is
`self_signed_tls_client_auth` (RFC 8705 §2.2) — a corner case
where the issuer accepts the client's self-signed certificate
without a CA chain.

Both mTLS variants present the same TLS client cert during the
handshake; the wire format is identical. The distinction lives in
two places:

1. **Issuer trust model.** `tls_client_auth` requires the issuer
   to validate the cert against a configured CA chain;
   `self_signed_tls_client_auth` requires the issuer to match the
   cert directly against a pre-registered cert thumbprint or
   public-key fingerprint.
2. **Discovery-list entry.** Issuers advertise which trust model
   they support via separate
   `introspection_endpoint_auth_methods_supported` entries. An
   operator deploying against an issuer that only advertises
   `self_signed_tls_client_auth` needs a builder that flips the
   auth method to that variant so the auto-selector matches.

Phase 25 wraps up the auth-method surface. After Phase 25 tako
covers six of six RFC 7662 §2.1 / RFC 8414 / RFC 8705-listed
introspection auth methods. This is the natural close-out of the
~10-phase OIDC hardening arc that started with Phase 14.B.

(Naming note: a previous `PLAN_PHASE25.md` file held Phase 2.5's
plan because of the `2.5 → 25` reading. Renamed to
`PLAN_PHASE2_5.md` in this commit so Phase 25 can claim the slot.)

**Theme:** *Close the OIDC introspection auth-method surface to
all six published variants.*

**Tag:** v0.26.0.

## A. `IntrospectionAuthMethod::SelfSignedTlsClientAuth` variant

### A.1 — Public surface

[`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs):

```rust
pub enum IntrospectionAuthMethod {
    #[default]
    ClientSecretBasic,
    ClientSecretPost,
    ClientSecretJwt,
    PrivateKeyJwt,
    TlsClientAuth,
    /// Phase 25 — RFC 8705 §2.2 self-signed mTLS. Wire-identical
    /// to [`Self::TlsClientAuth`] (both present a TLS client cert
    /// during the handshake), but the issuer matches the cert
    /// directly against a pre-registered cert thumbprint or
    /// public-key fingerprint instead of validating against a CA
    /// chain.
    ///
    /// Requires [`IntrospectionConfig::mtls_client`] to be
    /// configured (same as `TlsClientAuth`); errors at request
    /// time if missing.
    SelfSignedTlsClientAuth,
}
```

The `mtls_client` field on `IntrospectionConfig` carries the
Identity for both variants; no new field needed. Both variants
build the same `reqwest::Client` via
`reqwest::Identity::from_pem`.

### A.2 — Builder

```rust
impl OidcAuthResolver {
    /// Phase 25 — load a client cert + private key, build an
    /// mTLS-enabled client, and switch the introspection auth
    /// method to RFC 8705 §2.2 `self_signed_tls_client_auth`.
    /// Identical wire shape to [`Self::with_introspection_mtls`];
    /// the only difference is the `auth_method` enum variant
    /// (which determines which discovery-list entry the
    /// auto-selector matches).
    pub fn with_introspection_self_signed_mtls(
        mut self,
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> Result<Self, TakoError> { ... }
}
```

A combined-PEM convenience method
(`with_introspection_self_signed_mtls_combined`) ships alongside,
mirroring the Phase 24.A pattern.

### A.3 — Auto-selector extension

The Phase 24 five-tier preference order extends to a six-tier
ordering:

```
tls_client_auth (CA-backed; mTLS identity present)
  > self_signed_tls_client_auth (self-signed; mTLS identity present)
  > private_key_jwt (asymmetric key present)
  > client_secret_jwt (symmetric secret present)
  > client_secret_basic
  > client_secret_post
```

Rationale: CA-backed is stronger than self-signed because the CA
chain provides ongoing trust validation (revocation lists, etc.).
Both gated on having an mTLS identity configured.

### A.4 — `introspect()` branch

`introspect()` already handles `TlsClientAuth` by swapping to
`cfg.mtls_client` and skipping body credentials. Extend to treat
`SelfSignedTlsClientAuth` identically — the wire shape is
identical, only the conceptual trust model differs.

### A.5 — Tests

Five new unit tests in
[`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs):

1. `with_introspection_self_signed_mtls_accepts_valid_pem` — happy
   path; `auth_method` set to `SelfSignedTlsClientAuth`;
   `mtls_client` is `Some`.
2. `with_introspection_self_signed_mtls_combined_accepts_concatenated_pem`.
3. `with_introspection_self_signed_mtls_rejects_garbage_pem` — PEM
   parse failure surfaces at builder time.
4. `auto_select_prefers_tls_client_auth_over_self_signed_when_both_listed` —
   discovery advertises both; CA-backed wins.
5. `auto_select_picks_self_signed_when_only_self_signed_listed` —
   `tls_client_auth` not in the advertised list, but
   `self_signed_tls_client_auth` is.

The Phase 24 mTLS test cert + key fixtures are reused.

## B. Python facade

### B.1 — `OidcAuth.with_introspection_self_signed_mtls`

[`crates/tako-py/src/py_compat.rs`](crates/tako-py/src/py_compat.rs):
two new builder methods mirroring the Rust API.

### B.2 — Alias parser update

`with_introspection_auth_method(method)` accepts new
case-insensitive aliases mapping to `SelfSignedTlsClientAuth`:
- `"self_signed_tls_client_auth"` (RFC 8705 §2.2 spec name)
- `"self-signed-tls-client-auth"` (kebab variant)
- `"self_signed_mtls"` / `"self-signed-mtls"` (operator-friendly)

### B.3 — Module docstring update

[`python/tako/compat.py`](python/tako/compat.py): mention the new
builders + aliases.

### B.4 — Tests

[`tests/python/test_phase25_self_signed_mtls.py`](tests/python/test_phase25_self_signed_mtls.py):
facade attribute presence + module docstring smoke.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  self-signed mTLS tests in 25.A.5 pass; existing Phase 24 mTLS
  tests still byte-for-byte green.
- `pytest -q tests/python/test_phase25_self_signed_mtls.py` —
  green.

## Out of scope (Phase 26+)

- **OIDC mTLS end-to-end integration test** — real TLS server
  requiring client auth; needs ~300 lines of test infra
  (axum-server + rustls + per-test CA). Deferred.
- **OIDC mTLS cert / key rotation** — long-running deployments
  rotating client certs would need a refresh mechanism. Deferred
  pending a real-world ask.
- **URL-source images for Bedrock / Ollama** — both need
  tako-side pre-fetch with an SSRF guard.
- **Vertex File API upload flow.**
- **Eval harness real graders** (SWE-Bench Lite, GPQA Diamond).
- **OIDC refresh-token / revocation-endpoint flows.**
- **`ChainedAuthResolver` short-circuit semantics.**

## Commits

1. `chore: rename PLAN_PHASE25.md → PLAN_PHASE2_5.md to free the Phase 25 slot`
2. `feat(tako-compat): self_signed_tls_client_auth introspection auth method (Phase 25.A)`
3. `feat(tako-py): self_signed_tls_client_auth Python facade (Phase 25.B)`
4. `docs: Phase 25 PLAN/README/CHANGELOG flip (v0.26.0)`
