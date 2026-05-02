//! Phase 42 — OIDC mTLS end-to-end integration tests.
//!
//! Closes the loop on Phases 24 / 25 / 33 / 35 / 37 / 39: builder-level
//! tests prove PEM parsing succeeds and the auth-method enum flips
//! correctly, but until now no test ever performed a real mTLS
//! handshake against a server requiring CA-backed client cert auth.
//! The PLAN.md backlog flagged this gap explicitly.
//!
//! Each test:
//!   1. Generates a fresh CA + server leaf + client leaf via `rcgen`.
//!   2. Spins up an `axum-server` running TLS via `rustls` configured
//!      with `WebPkiClientVerifier` (`RequireAndVerifyClientCert`
//!      semantics) — only client certs signed by the per-test CA are
//!      accepted.
//!   3. Spins up a plain-HTTP `wiremock` server hosting the OIDC
//!      discovery doc + JWKS (the resolver's default HTTP client
//!      doesn't trust the test CA, so discovery / JWKS run over HTTP
//!      while introspection runs over mTLS HTTPS — production-shape).
//!   4. Builds an `OidcAuthResolver` via `discover()` →
//!      `with_introspection_uri()` →
//!      `with_introspection_mtls_extra_root()` so the resolver's mTLS
//!      client trusts the per-test CA.
//!   5. Signs a per-test JWT with the JWKS-published RS256 key and
//!      runs the full `resolve(token)` path — every layer (discovery
//!      cache, JWKS lookup, JWT validation, mTLS handshake, RFC 7662
//!      introspection POST + response parse) is exercised on the wire.
//!
//! No shared fixtures: each test is self-contained, regenerates
//! certs, and binds a fresh port. Two parallel test runs cannot
//! collide.

#![cfg(feature = "oidc")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::{Arc, Once};
use std::time::Duration;

use axum::{Router, extract::Form, http::StatusCode, response::IntoResponse, routing::post};
use axum_server::tls_rustls::RustlsConfig;
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
};
use rsa::rand_core::OsRng;
use rsa::{RsaPrivateKey, RsaPublicKey, pkcs8::EncodePrivateKey, traits::PublicKeyParts};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use serde::Deserialize;
use serde_json::{Value, json};
use tako_compat::{AuthResolver, OidcAuthResolver};
use tako_core::TakoError;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// rustls crypto provider — install exactly once per test binary.
// ---------------------------------------------------------------------------
//
// `rcgen` pulls in `aws-lc-rs`; the workspace `reqwest` pulls in `rustls`
// with the `aws_lc_rs` feature; the test fixture pins `rustls` directly
// with `aws_lc_rs`. With multiple linked providers, rustls 0.23 cannot
// auto-pick a default — install one up-front.
fn ensure_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

// ---------------------------------------------------------------------------
// Per-test cert fixture: CA + server leaf (SAN = localhost) + client leaf.
// ---------------------------------------------------------------------------

struct CertSet {
    ca_pem: String,
    server_cert_pem: String,
    server_key_pem: String,
    client_cert_pem: String,
    client_key_pem: String,
}

fn build_certs() -> CertSet {
    ensure_crypto_provider();

    // CA (self-signed root)
    let ca_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name = DistinguishedName::new();
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "tako-test-ca");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca_params.self_signed(&ca_kp).unwrap();
    let ca_pem = ca_cert.pem();
    let ca_issuer: Issuer<'_, _> = Issuer::from_params(&ca_params, &ca_kp);

    // Server leaf (signed by CA, SAN = localhost)
    let server_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut server_params = CertificateParams::default();
    server_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    server_params.subject_alt_names = vec![SanType::DnsName("localhost".try_into().unwrap())];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_cert = server_params.signed_by(&server_kp, &ca_issuer).unwrap();
    let server_cert_pem = server_cert.pem();
    let server_key_pem = server_kp.serialize_pem();

    // Client leaf (signed by CA, EKU = ClientAuth)
    let client_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut client_params = CertificateParams::default();
    client_params
        .distinguished_name
        .push(DnType::CommonName, "tako-test-client");
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_cert = client_params.signed_by(&client_kp, &ca_issuer).unwrap();
    let client_cert_pem = client_cert.pem();
    let client_key_pem = client_kp.serialize_pem();

    CertSet {
        ca_pem,
        server_cert_pem,
        server_key_pem,
        client_cert_pem,
        client_key_pem,
    }
}

// ---------------------------------------------------------------------------
// Per-test RS256 JWT keypair + JWKS publication helpers.
// ---------------------------------------------------------------------------

struct JwtKeys {
    private_pem: String,
    /// JWK `n` parameter, base64url-no-pad encoded.
    n_b64: String,
    /// JWK `e` parameter, base64url-no-pad encoded.
    e_b64: String,
}

fn build_jwt_keys() -> JwtKeys {
    let mut rng = OsRng;
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("RSA-2048 keygen");
    let pub_key = RsaPublicKey::from(&priv_key);

    let private_pem = priv_key
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("PKCS#8 PEM serialize")
        .to_string();

    let n_bytes = pub_key.n().to_bytes_be();
    let e_bytes = pub_key.e().to_bytes_be();
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let n_b64 = engine.encode(&n_bytes);
    let e_b64 = engine.encode(&e_bytes);

    JwtKeys {
        private_pem,
        n_b64,
        e_b64,
    }
}

fn build_jwks(keys: &JwtKeys, kid: &str) -> Value {
    json!({
        "keys": [{
            "kty": "RSA",
            "kid": kid,
            "alg": "RS256",
            "use": "sig",
            "n": keys.n_b64,
            "e": keys.e_b64,
        }]
    })
}

fn sign_test_jwt(keys: &JwtKeys, kid: &str, issuer: &str, audience: &str, sub: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.into());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = json!({
        "iss": issuer,
        "aud": audience,
        "sub": sub,
        // Resolver default tenant_claim is "tenant_id"; supply a
        // value so `principal_from_claims` succeeds.
        "tenant_id": "test-tenant",
        "exp": now + 300,
        "iat": now,
    });
    let key =
        EncodingKey::from_rsa_pem(keys.private_pem.as_bytes()).expect("RS256 EncodingKey from PEM");
    encode(&header, &claims, &key).expect("JWT encode")
}

// ---------------------------------------------------------------------------
// HTTPS axum-server bound to 127.0.0.1:0 with a rustls client-cert
// verifier rooted at the per-test CA. Returns the URL after the bind
// completes so the caller can configure the resolver.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct IntrospectForm {
    token: String,
    #[allow(dead_code)]
    token_type_hint: Option<String>,
}

async fn introspect_handler(Form(form): Form<IntrospectForm>) -> impl IntoResponse {
    // The resolver always sends the token in the form body; we don't
    // assert anything about the auth method here (the server side of
    // RFC 8705 §2 is the TLS handshake, not body content).
    if form.token.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing token").into_response();
    }
    let body = json!({
        "active": true,
        "sub": "alice",
        "aud": "test-audience",
        "iss": "https://test-issuer/",
        "exp": 9_999_999_999i64,
    });
    (StatusCode::OK, axum::Json(body)).into_response()
}

async fn spawn_mtls_introspect_server(certs: &CertSet) -> String {
    ensure_crypto_provider();

    // Server cert chain + key parsed via the maintained
    // `rustls::pki_types::pem::PemObject` trait (avoids the
    // unmaintained `rustls-pemfile` crate, RUSTSEC-2025-0134).
    let server_certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(certs.server_cert_pem.as_bytes())
            .collect::<Result<_, _>>()
            .expect("server certs parse");
    let server_key =
        PrivateKeyDer::from_pem_slice(certs.server_key_pem.as_bytes()).expect("server key parses");

    // Trust root for client cert verification — the per-test CA.
    let mut root_store = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(certs.ca_pem.as_bytes()) {
        root_store
            .add(cert.expect("CA cert parses"))
            .expect("CA cert added");
    }
    let client_verifier = WebPkiClientVerifier::builder(Arc::new(root_store))
        .build()
        .expect("client verifier builds");

    let server_config = ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(server_certs, server_key)
        .expect("server config builds");

    let rustls_config = RustlsConfig::from_config(Arc::new(server_config));

    // Bind to a random port on 127.0.0.1; the SAN is `localhost` so
    // we must connect via `https://localhost:<port>/`. Resolution
    // happens client-side (reqwest → tokio dns → 127.0.0.1).
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    // axum-server hands the listener to tokio, which requires
    // non-blocking mode (otherwise tokio panics at registration
    // time on macOS / Linux per tokio-rs/tokio#7172).
    std_listener.set_nonblocking(true).unwrap();
    let local_addr: SocketAddr = std_listener.local_addr().unwrap();
    let url = format!("https://localhost:{}", local_addr.port());

    let app: Router = Router::new().route("/introspect", post(introspect_handler));

    let server =
        axum_server::from_tcp_rustls(std_listener, rustls_config).expect("from_tcp_rustls");
    tokio::spawn(async move {
        server
            .serve(app.into_make_service())
            .await
            .expect("axum-server runs");
    });

    // Tiny grace period so the bind+listen completes before the test
    // hits it. `axum-server` doesn't expose a "ready" signal.
    tokio::time::sleep(Duration::from_millis(50)).await;
    url
}

// ---------------------------------------------------------------------------
// Plain-HTTP wiremock for OIDC discovery + JWKS publication. Returns
// the issuer URL after both routes are mounted.
// ---------------------------------------------------------------------------

async fn spawn_oidc_issuer(
    keys: &JwtKeys,
    kid: &str,
    introspect_uri: &str,
) -> (MockServer, String) {
    let server = MockServer::start().await;
    let issuer_url = server.uri();

    // Discovery doc — issuer must round-trip exactly so `discover()`
    // accepts it.
    let discovery = json!({
        "issuer": &issuer_url,
        "jwks_uri": format!("{issuer_url}/jwks"),
        "introspection_endpoint": introspect_uri,
        "introspection_endpoint_auth_methods_supported": [
            "tls_client_auth",
            "self_signed_tls_client_auth",
            "client_secret_basic",
        ],
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(build_jwks(keys, kid)))
        .mount(&server)
        .await;

    (server, issuer_url)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mtls_round_trip_succeeds_with_extra_root() {
    let certs = build_certs();
    let jwt_keys = build_jwt_keys();
    let kid = "test-key";

    let mtls_url = spawn_mtls_introspect_server(&certs).await;
    let introspect_uri = format!("{mtls_url}/introspect");
    let (_issuer_mock, issuer_url) = spawn_oidc_issuer(&jwt_keys, kid, &introspect_uri).await;

    let resolver = OidcAuthResolver::discover(&issuer_url, "test-audience")
        .await
        .expect("discover")
        .with_introspection_uri(&introspect_uri, "client", None)
        .with_introspection_mtls_extra_root(
            certs.client_cert_pem.as_bytes(),
            certs.client_key_pem.as_bytes(),
            certs.ca_pem.as_bytes(),
        )
        .expect("with_introspection_mtls_extra_root");

    let token = sign_test_jwt(&jwt_keys, kid, &issuer_url, "test-audience", "alice");
    let principal = resolver
        .resolve(&token)
        .await
        .expect("resolve should succeed end-to-end (handshake + introspect)");
    assert_eq!(principal.user_id, "alice");
}

#[tokio::test]
async fn mtls_handshake_fails_when_client_cert_missing() {
    // Same server (`RequireAndVerifyClientCert`), but the resolver
    // is wired through the **non-mTLS** path: `with_introspection`
    // builds a `ClientSecretBasic` config; the resolver's default
    // HTTP client has no client identity. The TLS handshake to the
    // server's introspect endpoint must abort.
    let certs = build_certs();
    let jwt_keys = build_jwt_keys();
    let kid = "test-key";

    let mtls_url = spawn_mtls_introspect_server(&certs).await;
    let introspect_uri = format!("{mtls_url}/introspect");
    let (_issuer_mock, issuer_url) = spawn_oidc_issuer(&jwt_keys, kid, &introspect_uri).await;

    // Resolver's default client doesn't trust the test CA either,
    // so the handshake fails for two reasons (no client cert AND
    // unknown server CA). Either failure proves the server side
    // enforces the policy; we only assert that `resolve` returns
    // an error before introspection succeeds.
    let resolver = OidcAuthResolver::discover(&issuer_url, "test-audience")
        .await
        .expect("discover")
        .with_introspection_uri(&introspect_uri, "client", Some("secret".into()));

    let token = sign_test_jwt(&jwt_keys, kid, &issuer_url, "test-audience", "alice");
    let err = resolver
        .resolve(&token)
        .await
        .expect_err("resolve should fail because the introspect TLS handshake aborts");
    let msg = format!("{err:?}");
    // Could be Transport (handshake / connection error) or Invalid
    // (bad-status response). The key signal is that resolve did not
    // succeed.
    assert!(
        matches!(err, TakoError::Transport(_) | TakoError::Invalid(_)),
        "unexpected error variant: {msg}"
    );
}

#[tokio::test]
async fn mtls_handshake_fails_when_extra_root_not_configured() {
    // Resolver wired through the mTLS path with a valid client
    // identity, but **without** the `_extra_root` builder — the
    // resolver's mTLS client trusts only the system + webpki-roots
    // store, so it will reject the server's privately-issued cert
    // at handshake time.
    let certs = build_certs();
    let jwt_keys = build_jwt_keys();
    let kid = "test-key";

    let mtls_url = spawn_mtls_introspect_server(&certs).await;
    let introspect_uri = format!("{mtls_url}/introspect");
    let (_issuer_mock, issuer_url) = spawn_oidc_issuer(&jwt_keys, kid, &introspect_uri).await;

    let resolver = OidcAuthResolver::discover(&issuer_url, "test-audience")
        .await
        .expect("discover")
        .with_introspection_uri(&introspect_uri, "client", None)
        .with_introspection_mtls(
            certs.client_cert_pem.as_bytes(),
            certs.client_key_pem.as_bytes(),
        )
        .expect("with_introspection_mtls");

    let token = sign_test_jwt(&jwt_keys, kid, &issuer_url, "test-audience", "alice");
    let err = resolver
        .resolve(&token)
        .await
        .expect_err("resolve should fail: server cert is not trusted by the default root store");
    assert!(
        matches!(err, TakoError::Transport(_)),
        "expected a Transport error (TLS handshake), got: {err:?}"
    );
}

#[tokio::test]
async fn self_signed_mtls_extra_root_round_trip_succeeds() {
    // Same wire shape as the happy-path test, but using the RFC 8705
    // §2.2 `self_signed_tls_client_auth` builder. The server side
    // can't actually distinguish — both methods present a TLS
    // client cert. The point is to exercise the second public
    // builder.
    let certs = build_certs();
    let jwt_keys = build_jwt_keys();
    let kid = "test-key";

    let mtls_url = spawn_mtls_introspect_server(&certs).await;
    let introspect_uri = format!("{mtls_url}/introspect");
    let (_issuer_mock, issuer_url) = spawn_oidc_issuer(&jwt_keys, kid, &introspect_uri).await;

    let resolver = OidcAuthResolver::discover(&issuer_url, "test-audience")
        .await
        .expect("discover")
        .with_introspection_uri(&introspect_uri, "client", None)
        .with_introspection_self_signed_mtls_extra_root(
            certs.client_cert_pem.as_bytes(),
            certs.client_key_pem.as_bytes(),
            certs.ca_pem.as_bytes(),
        )
        .expect("with_introspection_self_signed_mtls_extra_root");

    let token = sign_test_jwt(&jwt_keys, kid, &issuer_url, "test-audience", "alice");
    let principal = resolver.resolve(&token).await.expect("resolve");
    assert_eq!(principal.user_id, "alice");
}

// ---------------------------------------------------------------------------
// Phase 44 — discover() over HTTPS with private CA.
//
// Spawns an HTTPS axum-server (NO client-cert verification — just plain
// TLS with the per-test CA) hosting the OIDC discovery doc + JWKS. The
// new `discover_with_extra_root` constructor must accept the per-test
// CA bundle and complete discovery without TLS verification failures.
// ---------------------------------------------------------------------------

async fn discovery_get_handler(
    axum::extract::State(state): axum::extract::State<Arc<DiscoveryState>>,
) -> impl IntoResponse {
    (StatusCode::OK, axum::Json(state.discovery_doc.clone())).into_response()
}

async fn jwks_get_handler(
    axum::extract::State(state): axum::extract::State<Arc<DiscoveryState>>,
) -> impl IntoResponse {
    (StatusCode::OK, axum::Json(state.jwks.clone())).into_response()
}

#[derive(Clone)]
struct DiscoveryState {
    discovery_doc: Value,
    jwks: Value,
}

async fn spawn_https_oidc_issuer(certs: &CertSet, keys: &JwtKeys, kid: &str) -> String {
    ensure_crypto_provider();

    let server_certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(certs.server_cert_pem.as_bytes())
            .collect::<Result<_, _>>()
            .expect("server certs parse");
    let server_key =
        PrivateKeyDer::from_pem_slice(certs.server_key_pem.as_bytes()).expect("server key parses");

    // Plain TLS — no client cert verification (this server is just
    // discovery + JWKS, not introspection).
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(server_certs, server_key)
        .expect("server config builds");
    let rustls_config = RustlsConfig::from_config(Arc::new(server_config));

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let local_addr: SocketAddr = std_listener.local_addr().unwrap();
    let issuer_url = format!("https://localhost:{}", local_addr.port());

    let discovery = json!({
        "issuer": &issuer_url,
        "jwks_uri": format!("{issuer_url}/jwks"),
        "introspection_endpoint_auth_methods_supported": ["client_secret_basic"],
    });
    let state = Arc::new(DiscoveryState {
        discovery_doc: discovery,
        jwks: build_jwks(keys, kid),
    });

    let app: Router = Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(discovery_get_handler),
        )
        .route("/jwks", axum::routing::get(jwks_get_handler))
        .with_state(state);

    let server =
        axum_server::from_tcp_rustls(std_listener, rustls_config).expect("from_tcp_rustls");
    tokio::spawn(async move {
        server
            .serve(app.into_make_service())
            .await
            .expect("axum-server runs");
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    issuer_url
}

#[tokio::test]
async fn discover_over_https_with_private_ca_succeeds() {
    // Phase 44 — the resolver-wide HTTP client must trust the
    // per-test private CA so that BOTH the discovery GET (during
    // construction) AND the JWKS GET (during `resolve`) succeed
    // against the HTTPS issuer.
    let certs = build_certs();
    let jwt_keys = build_jwt_keys();
    let kid = "test-key";

    let issuer_url = spawn_https_oidc_issuer(&certs, &jwt_keys, kid).await;

    let resolver = OidcAuthResolver::discover_with_extra_root(
        &issuer_url,
        "test-audience",
        certs.ca_pem.as_bytes(),
    )
    .await
    .expect("discover_with_extra_root over HTTPS-with-private-CA");

    // Sign + resolve a token: this exercises the JWKS GET on the
    // same private-CA server, proving the trust anchor flows
    // through to the JWKS path too (single shared `http` client).
    let token = sign_test_jwt(&jwt_keys, kid, &issuer_url, "test-audience", "alice");
    let principal = resolver
        .resolve(&token)
        .await
        .expect("resolve over JWKS-on-private-CA-issuer");
    assert_eq!(principal.user_id, "alice");
}

#[tokio::test]
async fn discover_over_https_without_extra_root_fails() {
    // Same HTTPS-with-private-CA issuer, but using the default
    // `discover()` constructor — the resolver-wide client trusts
    // only the system + webpki-roots store, so the discovery GET
    // must fail at TLS verification time.
    let certs = build_certs();
    let jwt_keys = build_jwt_keys();
    let kid = "test-key";

    let issuer_url = spawn_https_oidc_issuer(&certs, &jwt_keys, kid).await;

    let err = OidcAuthResolver::discover(&issuer_url, "test-audience")
        .await
        .expect_err("discovery should fail: server cert is not trusted by default");
    assert!(
        matches!(err, TakoError::Transport(_)),
        "expected Transport (TLS handshake), got: {err:?}"
    );
}

#[tokio::test]
async fn discover_with_extra_root_unparseable_pem_errors_at_constructor_time() {
    // Phase 44 fail-closed contract — garbage CA bytes must
    // surface as `Invalid` at construction time, before any
    // network call. (No issuer URL needed; we never reach the
    // GET.) Mirrors the
    // `extra_root_unparseable_pem_errors_at_builder_time` test
    // for the introspection mTLS path.
    let err = OidcAuthResolver::discover_with_extra_root(
        "https://issuer.example",
        "test-audience",
        b"definitely not a pem certificate",
    )
    .await
    .expect_err("garbage CA PEM should fail synchronously");
    let msg = format!("{err:?}");
    assert!(
        matches!(err, TakoError::Invalid(_))
            && (msg.contains("resolver extra root CA PEM bundle")
                || msg.contains("parsed zero certificates")),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn extra_root_unparseable_pem_errors_at_builder_time() {
    // No mTLS server needed — this asserts the builder fails-closed
    // synchronously when given garbage CA bytes.
    let certs = build_certs();
    let jwt_keys = build_jwt_keys();
    let kid = "test-key";
    let (_issuer_mock, issuer_url) =
        spawn_oidc_issuer(&jwt_keys, kid, "https://localhost:1/introspect").await;

    // Non-PEM bytes (no `-----BEGIN ... -----` markers) — `reqwest`'s
    // `Certificate::from_pem_bundle` parses zero certs and our
    // explicit empty-bundle guard fires.
    let err = OidcAuthResolver::discover(&issuer_url, "test-audience")
        .await
        .expect("discover")
        .with_introspection_uri("https://localhost:1/introspect", "client", None)
        .with_introspection_mtls_extra_root(
            certs.client_cert_pem.as_bytes(),
            certs.client_key_pem.as_bytes(),
            b"definitely not a pem certificate",
        )
        .expect_err("garbage CA PEM should fail at builder time");
    let msg = format!("{err:?}");
    assert!(
        matches!(err, TakoError::Invalid(_))
            && (msg.contains("mTLS extra root CA PEM bundle")
                || msg.contains("parsed zero certificates")),
        "unexpected error: {msg}"
    );
}
