# PLAN — Phase 24 (OIDC introspection mTLS / `tls_client_auth`)

## Context

Phase 16.B.2 (v0.17.0, 2026-05-01) shipped the OIDC introspection
`client_secret_post` auth method and explicitly deferred mTLS
(`tls_client_auth` / `self_signed_tls_client_auth`) with the
framing "needs reqwest TLS feature changes at workspace scope".

That framing turned out to be wrong: the workspace's existing
`reqwest = { features = ["rustls", "webpki-roots", ...] }` already
exposes [`reqwest::Identity::from_pem`] (verified by a probe
compile). Phase 24 can implement mTLS introspection without any
workspace-level dep change — just adapter-side wiring on
`tako-compat`.

Phase 17.A's auto-selector already handles `tls_client_auth` by
falling through to the fail-closed branch when no method is
mutually supported. Phase 24 turns this from "always fail-closed"
into "select if identity configured" — same shape as Phase 18.A
did for `private_key_jwt`.

After Phase 24 the OIDC introspection auth-method surface
covers all five RFC 7662 §2.1 / RFC 8414-listed methods we
intend to ship: `client_secret_basic` (default), `_post`, `_jwt`
(symmetric, Phase 17.B), `private_key_jwt` (asymmetric, Phase
18.A), and `tls_client_auth` (this phase). `self_signed_tls_client_auth`
remains deferred — it's a corner case where the issuer accepts
self-signed certs without a CA chain; identical wire shape to
`tls_client_auth` so the same builder works, but it warrants a
distinct discovery-list entry.

**Theme:** *Close the OIDC introspection mTLS gap that's been
deferred since Phase 16.*

**Tag:** v0.25.0.

## A. tako-compat mTLS wiring

### A.1 — Public surface

[`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs):

```rust
pub enum IntrospectionAuthMethod {
    #[default]
    ClientSecretBasic,
    ClientSecretPost,
    ClientSecretJwt,
    PrivateKeyJwt,
    /// Phase 24 — mTLS authentication. The client presents a
    /// TLS certificate during the introspection-endpoint
    /// handshake; the issuer matches the cert's subject DN /
    /// SAN against the configured `client_id`. No body
    /// credential, no Authorization header, no JWT.
    ///
    /// Requires
    /// [`IntrospectionConfig::mtls_client`] to be configured;
    /// errors at request time if missing.
    TlsClientAuth,
}

pub struct IntrospectionConfig {
    // ... existing fields
    /// Phase 24 — mTLS-enabled HTTP client for
    /// `TlsClientAuth`. Built eagerly at builder time
    /// (`with_introspection_mtls`) so PEM parsing failures
    /// surface early. `Arc<Client>` because [`reqwest::Client`]
    /// is already internally Arc'd; cloning is cheap.
    pub mtls_client: Option<Arc<reqwest::Client>>,
}
```

### A.2 — Builders

```rust
impl OidcAuthResolver {
    /// Phase 24 — load a client cert + private key from
    /// separate PEM blobs and switch the introspection auth
    /// method to `tls_client_auth`. Returns a NEW
    /// `OidcAuthResolver` (immutable builder; matches the
    /// `with_introspection_jwt_*_pem` cadence). PEM parse
    /// failure surfaces as `TakoError::Invalid` at builder
    /// time.
    ///
    /// `cert_pem` should be a PEM-encoded X.509 certificate
    /// (or chain — `reqwest::Identity::from_pem` accepts
    /// concatenated certs). `key_pem` should be a PKCS#8 or
    /// SEC1-encoded private key matching the cert.
    pub fn with_introspection_mtls(
        mut self,
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> Result<Self, TakoError> {
        let identity = build_mtls_identity(cert_pem, key_pem)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .identity(identity)
            .build()
            .map_err(|e| TakoError::Invalid(format!(
                "oidc: failed to build mTLS client: {e}"
            )))?;
        if let Some(cfg) = self.introspection.as_mut() {
            cfg.mtls_client = Some(Arc::new(client));
            cfg.auth_method = IntrospectionAuthMethod::TlsClientAuth;
        }
        Ok(self)
    }

    /// Phase 24 — convenience: load cert + key from a single
    /// concatenated PEM blob (the common output format from
    /// `cat cert.pem key.pem` or `openssl pkcs12 -nokeys`).
    /// `reqwest::Identity::from_pem` accepts the combined form.
    pub fn with_introspection_mtls_combined(
        self,
        combined_pem: &[u8],
    ) -> Result<Self, TakoError> {
        // Single-arg path delegates to the two-arg path with
        // the same blob passed twice; reqwest's parser locates
        // the cert and key blocks by PEM section markers
        // independently.
        self.with_introspection_mtls(combined_pem, combined_pem)
    }
}

fn build_mtls_identity(cert_pem: &[u8], key_pem: &[u8]) -> Result<reqwest::Identity, TakoError> {
    // reqwest's `from_pem` requires the cert + key in one blob.
    // Concatenate caller's separate pieces.
    let mut combined = Vec::with_capacity(cert_pem.len() + key_pem.len() + 1);
    combined.extend_from_slice(cert_pem);
    if !cert_pem.ends_with(b"\n") {
        combined.push(b'\n');
    }
    combined.extend_from_slice(key_pem);
    reqwest::Identity::from_pem(&combined).map_err(|e| {
        TakoError::Invalid(format!("oidc: invalid mTLS identity PEM: {e}"))
    })
}
```

### A.3 — Auto-selector extension

The Phase 18.A four-tier auto-selector
([`oidc.rs:339-389`](crates/tako-compat/src/auth/oidc.rs#L339-L389))
already handles `private_key_jwt`. Extend to a five-tier
preference order with `tls_client_auth` at the head when an
mTLS identity is configured:

```
tls_client_auth (mTLS identity present)
  > private_key_jwt (asymmetric key present)
  > client_secret_jwt (symmetric secret present)
  > client_secret_basic
  > client_secret_post
```

Rationale: mTLS is the strongest authentication method (the
private key never leaves the client; the cert binds to a DN /
SAN the issuer pre-registered). Prefer it when both sides
support it.

### A.4 — `introspect()` branch

When `cfg.auth_method == TlsClientAuth`:

1. Require `cfg.mtls_client.is_some()` — else
   `TakoError::Invalid("oidc: tls_client_auth requires mtls_client to be set")`.
2. Use the mTLS client (not the resolver's default `self.http`)
   to send the introspection POST.
3. Body shape: same as Basic — `token=<jwt>&token_type_hint=access_token`,
   no `client_id` / `client_secret` / `client_assertion`. The
   issuer authenticates the client via the TLS handshake's cert,
   not the request body.
4. No `Authorization` header.

### A.5 — Tests

Six new unit tests in
[`crates/tako-compat/src/auth/oidc.rs`](crates/tako-compat/src/auth/oidc.rs):

1. `with_introspection_mtls_accepts_valid_pem` — accepts a
   valid self-signed cert + matching key from a test fixture;
   `auth_method` is set to `TlsClientAuth`; `mtls_client` is
   `Some`.
2. `with_introspection_mtls_rejects_garbage_pem` — invalid PEM
   surfaces as `TakoError::Invalid` at builder time (not a
   panic, not deferred).
3. `with_introspection_mtls_combined_accepts_concatenated_pem` —
   the convenience builder accepts cert + key concatenated.
4. `auto_select_prefers_tls_client_auth_when_listed_and_identity_present` —
   discovery advertises the full set; auto-selector picks
   `TlsClientAuth`.
5. `auto_select_skips_tls_client_auth_when_no_identity` — even
   when listed, falls back to the next supported method
   (`private_key_jwt` if present).
6. `introspect_tls_client_auth_errors_when_mtls_client_missing` —
   `auth_method` flipped to `TlsClientAuth` without configuring
   identity → request-time fail.

A test fixture (self-signed cert + key PKCS#8 PEM) is generated
once via `openssl` and embedded as a `static &[u8]` const in
the test module, matching the Phase 18.A pattern with the RSA
test keypair.

End-to-end mTLS-handshake tests (real TLS server requiring
client auth) need a TLS test fixture and are deferred to
Phase 25+. The actual mTLS connection is exercised in real
deployments.

## B. Python facade

### B.1 — `OidcAuth.with_introspection_mtls`

[`crates/tako-py/src/py_compat.rs`](crates/tako-py/src/py_compat.rs):

```rust
/// Phase 24 — load a client cert + private key from separate
/// PEM blobs and switch the introspection auth method to
/// `tls_client_auth`. Returns a NEW `OidcAuth`. Raises
/// `ValueError` on PEM parse failure.
fn with_introspection_mtls(
    &self,
    cert_pem: &[u8],
    key_pem: &[u8],
) -> PyResult<Self> {
    let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
    let r = cloned
        .with_introspection_mtls(cert_pem, key_pem)
        .map_err(map_err)?;
    Ok(PyOidcAuth { inner: Arc::new(r) })
}

/// Phase 24 — convenience for combined PEM blobs.
fn with_introspection_mtls_combined(
    &self,
    combined_pem: &[u8],
) -> PyResult<Self> { ... }
```

### B.2 — Alias parser update

`with_introspection_auth_method(method)` accepts the new
case-insensitive aliases `"tls_client_auth"` / `"tls-client-auth"`
/ `"mtls"`. Maps to
`IntrospectionAuthMethod::TlsClientAuth`.

### B.3 — Module docstring update

[`python/tako/compat.py`](python/tako/compat.py): mention the
new `with_introspection_mtls` builders + the `"tls_client_auth"` /
`"mtls"` alias.

### B.4 — Tests

[`tests/python/test_phase24_mtls.py`](tests/python/test_phase24_mtls.py):

1. `test_oidc_auth_has_with_introspection_mtls` — facade
   attribute presence.
2. `test_oidc_auth_has_with_introspection_mtls_combined`.
3. `test_with_introspection_auth_method_accepts_mtls_aliases` —
   four case-insensitive aliases.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  mTLS tests in 24.A.5 pass; existing Phase 16/17/18 OIDC
  introspection tests still byte-for-byte green.
- `pytest -q tests/python/test_phase24_mtls.py` — green on a
  wheel built with `--features auth-oidc`.

## Out of scope (Phase 25+)

- **`self_signed_tls_client_auth`** — RFC 8705 §2.2 corner case
  where the issuer accepts self-signed certs without a CA chain.
  Identical wire shape to `tls_client_auth`, so a single builder
  works — but the discovery-list entry is distinct, and the
  auto-selector preference may differ. Phase 25+.
- **End-to-end mTLS-handshake integration test** — needs a real
  TLS server requiring client auth (e.g. rustls-server in the
  test harness). Substantial test infra; the actual mTLS
  connection is exercised in real deployments.
- **Cert / key rotation** — Phase 24 builds the mTLS Client
  once at builder time; long-running deployments that rotate
  client certs would need a refresh mechanism. Phase 25+.
- URL-source images for Bedrock / Ollama, Vertex File API
  upload, eval-harness real graders, OIDC refresh-token /
  revocation, ChainedAuth short-circuit semantics — all
  carried over.

## Commits

1. `feat(tako-compat): mTLS introspection auth method (Phase 24.A)`
2. `feat(tako-py): OidcAuth mTLS Python facade (Phase 24.B)`
3. `docs: Phase 24 PLAN/README/CHANGELOG flip (v0.25.0)`
