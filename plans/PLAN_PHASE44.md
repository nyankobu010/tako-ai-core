# PLAN — Phase 44 (Operator-supplied root CA for OIDC discovery + JWKS)

> **Status: in progress.** Targets v0.45.0. Closes the
> "Custom CA support for non-introspection endpoints (JWKS,
> discovery)" carry-forward from Phase 42's out-of-scope
> section.

## Context

Phase 42 (v0.43.0) shipped operator-supplied CA support for
the OIDC **introspection** mTLS path:

```rust
OidcAuthResolver::with_introspection_mtls_extra_root(
    cert_pem, key_pem, extra_root_ca_pem,
)
```

Phase 43 (v0.44.0) wrapped that for Python.

The **resolver-wide** HTTP client (the one used by
`OidcAuthResolver::discover` for the OIDC discovery doc fetch
and by the JWKS refresh path) still has no CA injection
point. Concretely, [`crates/tako-compat/src/auth/oidc.rs:449-453`](crates/tako-compat/src/auth/oidc.rs#L449-L453):

```rust
pub async fn discover(issuer: &str, audience: &str) -> Result<Self, TakoError> {
    let http = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(...)?;
    ...
}
```

This `http` field then handles **both** the discovery doc
fetch (right above) and JWKS refreshes
([`oidc.rs:1304-1306`](crates/tako-compat/src/auth/oidc.rs#L1304-L1306)):

```rust
let resp = self
    .http
    .get(&self.jwks_uri)
    ...
```

Operators behind a private internal CA (Keycloak / Auth0
self-hosted / Authentik with corporate PKI) currently can't
even **boot** `OidcAuthResolver::discover` against their
issuer — the discovery GET fails TLS verification before the
resolver is even constructed, so they never get a chance to
call `with_introspection_mtls_extra_root`. Phase 42's
out-of-scope section flagged this:

> **Custom CA support for non-introspection endpoints (JWKS,
> discovery).** The default `OidcAuthResolver` HTTP client
> is built inside `discover()` with no public injection
> point. Adding a `with_extra_root_ca` builder for the
> resolver-wide client is a larger ask (touches discovery
> boot path, `discover()` signature, `Client` lifecycle).
> Defer until an operator asks.

Phase 44 closes the gap. The right shape is a parallel
constructor — not a post-construction builder — because the
trust anchor is needed *during* discovery itself, before any
`with_*` call has a chance to run.

## Why now

Phases 42 + 43 closed the operator-supplied-CA story for the
introspection path. A Keycloak-behind-private-CA shop
following the resulting recipe today still hits TLS
verification failures on the very first
`OidcAuth.discover(issuer, audience)` await — they never
reach the introspection-mTLS phase. Phase 44 closes the
last door on the "private-CA OIDC works end-to-end" story.

Same wire shape as the Phase 24 → 42 progression for the
introspection client: parse a PEM bundle via
`reqwest::Certificate::from_pem_bundle`, reject empty
bundles at construction time, additive trust on top of the
system + webpki-roots store.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 44.A | New `OidcAuthResolver::discover_with_extra_root(issuer, audience, extra_root_ca_pem)` async constructor | [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs) |
| 44.B | Internal `build_resolver_http_client(extra_root_ca_pem: Option<&[u8]>)` helper | [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs) |
| 44.C | Unit tests: happy + garbage PEM + empty-bundle (constructor-time fail-closed) | [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs) |
| 44.D | New e2e test: discovery + JWKS over HTTPS-with-private-CA | [`crates/tako-compat/tests/oidc_mtls_e2e.rs`](../crates/tako-compat/tests/oidc_mtls_e2e.rs) |
| 44.E | Workspace + Python version 0.44.0 → 0.45.0 | various |
| 44.F | PLAN.md row + Phase 45 candidate refresh | [`PLAN.md`](../PLAN.md) |
| 44.G | CHANGELOG.md `[0.45.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

Python facade for the new constructor stays out of scope for
this phase — same cadence as Phase 42 (Rust) → Phase 43
(Python facade). The Python wrap will land in Phase 45.

## What this phase will land

### 44.A — `discover_with_extra_root` constructor

```rust
/// Phase 44 — Same as [`Self::discover`] but builds the
/// resolver-wide HTTP client with an operator-supplied
/// PEM-encoded root CA bundle added to its trust store.
///
/// The `http` field handles BOTH the OIDC discovery doc
/// fetch (this constructor) AND the JWKS refresh path used
/// for live token validation, so the same trust anchor
/// covers both. Use this for enterprise self-hosted OIDC
/// issuers (Keycloak / Auth0 self-hosted / Authentik)
/// presenting a server cert signed by a private internal
/// CA — without this constructor, the discovery GET fails
/// TLS verification before the resolver is even returned.
///
/// `extra_root_ca_pem` accepts a single root cert or a
/// concatenated multi-cert PEM bundle. At least one cert
/// must parse; empty bundles error at construction time
/// (matches the Phase 42 introspection-mTLS contract).
///
/// Independent from
/// [`Self::with_introspection_mtls_extra_root`] — the
/// introspection mTLS client carries its own CA store.
/// Operators with one PKI for the whole stack pass the same
/// PEM bundle to both.
pub async fn discover_with_extra_root(
    issuer: &str,
    audience: &str,
    extra_root_ca_pem: &[u8],
) -> Result<Self, TakoError>;
```

Both `discover` and `discover_with_extra_root` delegate to
the same `discover_inner(issuer, audience, extra_root_ca_pem: Option<&[u8]>)`
function so the body of `discover()` stays one line:

```rust
pub async fn discover(issuer: &str, audience: &str) -> Result<Self, TakoError> {
    Self::discover_inner(issuer, audience, None).await
}

pub async fn discover_with_extra_root(
    issuer: &str,
    audience: &str,
    extra_root_ca_pem: &[u8],
) -> Result<Self, TakoError> {
    Self::discover_inner(issuer, audience, Some(extra_root_ca_pem)).await
}

async fn discover_inner(
    issuer: &str,
    audience: &str,
    extra_root_ca_pem: Option<&[u8]>,
) -> Result<Self, TakoError> { ... }
```

### 44.B — `build_resolver_http_client` helper

Mirrors the Phase 42
[`build_mtls_reqwest_client`](../crates/tako-compat/src/auth/oidc.rs#L1528)
shape but without the client cert / identity (this client
is plain TLS, not mTLS):

```rust
fn build_resolver_http_client(
    extra_root_ca_pem: Option<&[u8]>,
) -> Result<reqwest::Client, TakoError> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(10));
    if let Some(ca_pem) = extra_root_ca_pem {
        let extra_roots = reqwest::Certificate::from_pem_bundle(ca_pem).map_err(|e| {
            TakoError::Invalid(format!(
                "oidc: invalid resolver extra root CA PEM bundle: {e}"
            ))
        })?;
        if extra_roots.is_empty() {
            return Err(TakoError::Invalid(
                "oidc: resolver extra root CA PEM bundle parsed zero certificates".into(),
            ));
        }
        for cert in extra_roots {
            builder = builder.add_root_certificate(cert);
        }
    }
    builder
        .build()
        .map_err(|e| TakoError::Transport(format!("oidc: failed to build resolver http client: {e}")))
}
```

The `discover_inner` body uses this helper directly:

```rust
let http = build_resolver_http_client(extra_root_ca_pem)?;
```

Note: the existing `discover()` returns `TakoError::Transport`
on builder failure
([`oidc.rs:453`](crates/tako-compat/src/auth/oidc.rs#L453)),
so the helper mirrors that for the no-CA case. PEM parse
failures (a constructor-time, operator-input error) map to
`TakoError::Invalid` to match the Phase 42 introspection
contract — fail-closed at the operator boundary.

### 44.C — Unit tests

Three new tests in the existing `mod tests` block:

| Test | Assertion |
|------|-----------|
| `discover_with_extra_root_rejects_garbage_ca_pem` | Garbage CA bytes → `TakoError::Invalid` synchronously, before any network call. |
| `discover_with_extra_root_rejects_empty_ca_bundle` | Empty bytes → `TakoError::Invalid` (matches Phase 42 fail-closed contract). |
| `discover_with_extra_root_succeeds_against_wiremock_with_test_ca` | Reuses the existing `wiremock` discovery-doc test pattern, but with a real PEM-encoded CA bundle (a test cert from `rcgen` minted in the test) — proves the happy-path constructor wires the CA through to the underlying `reqwest::Client` correctly. The wiremock server itself runs over plain HTTP (cheap), so the CA doesn't have to actually be exercised in the handshake here — that's what the Phase 44.D e2e test covers. |

The third test uses `rcgen` (already a `tako-compat` dev-dep
since Phase 42) to mint a throwaway CA cert PEM rather than
embedding a hardcoded fixture.

### 44.D — New e2e test: discovery over HTTPS with private CA

Add **one** new test to
[`crates/tako-compat/tests/oidc_mtls_e2e.rs`](../crates/tako-compat/tests/oidc_mtls_e2e.rs):

`discover_over_https_with_private_ca_succeeds`:

1. rcgen-generates a per-test CA + an `axum-server` rustls
   server leaf signed by it.
2. Spins up a TLS-enabled `axum-server` (no client-cert
   verification — this server is just discovery + JWKS, not
   introspection) hosting:
   - `GET /.well-known/openid-configuration` — discovery doc.
   - `GET /jwks` — the RS256 public key as a JWK.
3. Builds the resolver via the **new**
   `OidcAuthResolver::discover_with_extra_root(https_issuer, audience, ca_pem)`
   constructor.
4. Asserts the resolver was constructed (no error) and that
   the discovered fields look right.
5. Optionally signs a JWT and runs `resolve(token)` to prove
   the JWKS GET also succeeds against the same private-CA
   server. (No introspection — that would just retread the
   Phase 42 e2e tests; the discovery + JWKS over HTTPS is
   the new path.)

Plus one negative test:

`discover_over_https_without_extra_root_fails`:

1. Same axum-server-with-private-CA setup.
2. Builds the resolver via the **default** `discover()`
   constructor (no CA injection).
3. Expects `TakoError::Transport` from the discovery GET
   itself — proves the gap this phase closes.

The existing five Phase 42 e2e tests stay as-is (plain
HTTP discovery is still a valid, simpler operator
configuration).

### 44.E — Version bump

0.44.0 → 0.45.0 across:
- workspace `Cargo.toml`
- internal crate version pins inside the same `Cargo.toml`
  (the 14 `tako-* = { path = ..., version = "0.44.0" }`
  rows; `sed`-replaceable as a single pass)
- `pyproject.toml`
- `python/tako/__init__.py`
- `tests/python/test_smoke.py`
- `Cargo.lock` regenerates as fallout

### 44.F — PLAN.md update

- New row `44 — Operator-supplied root CA for OIDC
  discovery + JWKS`.
- Phase 45 candidate list — leading with "Python facade for
  `discover_with_extra_root`" (Phase 42→43 cadence). Other
  carry-forward items (eval-harness real graders, OTel
  real-collector e2e, Vertex deterministic-per-call
  placeholder logic) stay flagged but un-pre-committed.

### 44.G — CHANGELOG `[0.45.0]`

Standard format, brief.

## Critical files

**Modified:**
- [`crates/tako-compat/src/auth/oidc.rs`](../crates/tako-compat/src/auth/oidc.rs)
  — new constructor + helper + 3 unit tests.
- [`crates/tako-compat/tests/oidc_mtls_e2e.rs`](../crates/tako-compat/tests/oidc_mtls_e2e.rs)
  — 2 new e2e tests.
- Standard PLAN/CHANGELOG/version flip:
  [`Cargo.toml`](../Cargo.toml),
  [`pyproject.toml`](../pyproject.toml),
  [`python/tako/__init__.py`](../python/tako/__init__.py),
  [`tests/python/test_smoke.py`](../tests/python/test_smoke.py),
  [`PLAN.md`](../PLAN.md),
  [`CHANGELOG.md`](../CHANGELOG.md).
- `Cargo.lock` (version-bump fallout only).

**Created:**
- [`plans/PLAN_PHASE44.md`](PLAN_PHASE44.md) (this file).

No new files outside of the plan itself — scope is tight.

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy -p tako-py --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider" -- -D warnings`.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
4. `cargo test -p tako-compat --all-features` — old 159 lib + 5 e2e + 8 server + 6 vault_token + 1 doctest, plus the 3 new unit + 2 new e2e tests.
5. `cargo test --workspace --exclude tako-py --all-features` — no regressions across other crates.
6. `ruff format --check` + `ruff check` (no Python changes other than smoke-test version).
7. `maturin develop --release --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider sigstore"` — wheel builds at v0.45.0.
8. `pytest -q` — full suite green (no facade changes; smoke test pins v0.45.0).

## Out of scope

- **Python facade for `discover_with_extra_root`.** Same
  Rust→Python cadence as Phases 42→43, 39→40, 37→38.
  Lands in Phase 45.
- **Post-construction `OidcAuthResolver::with_extra_root_ca` builder.**
  Edge case — would only matter if discovery were on a
  public-CA host but JWKS were on a private-CA host (or
  vice versa). Defer until ask; operators with one PKI
  pass the same bundle to the constructor.
- **Persistence of the CA bundle as a public field on
  `OidcAuthResolver`.** It's baked into the `http` client
  at construction time — the field already lives on the
  resolver. Exposing the raw bytes adds API surface for no
  current consumer.
- **CA support for the introspection JWKS / `client_assertion`
  paths.** Already covered by Phase 42's
  `with_introspection_mtls_extra_root` (introspection client
  carries its own CA). The `client_assertion` JWT signing
  paths are pure local crypto — no HTTP, no TLS.
- **Loading the CA bundle from a filesystem path / Vault
  secret / k8s ConfigMap.** Operators marshal those bytes
  themselves in their bootstrap. No-config wiring is a
  Phase 35-style watcher and a separate roadmap item.
