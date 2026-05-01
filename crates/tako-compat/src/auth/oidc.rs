//! `OidcAuthResolver` — discovers an OIDC provider's JWKS and validates
//! incoming ID tokens against it.
//!
//! Phase 14.B. Behaviour:
//! - **Discovery** runs once at construction via
//!   [`Self::discover`] (`<issuer>/.well-known/openid-configuration`).
//!   The `jwks_uri` is captured for later JWKS fetches.
//! - **JWKS cache** is `Arc<RwLock<JwkSet>>`, refreshed lazily when
//!   stale (`refresh_interval`, default 1h) or when a token's `kid` is
//!   absent from the cache. On signature failure the resolver
//!   force-refreshes once and retries — the documented mitigation for
//!   the JWKS-rotation race in `oauth2-rs`.
//! - **Token validation** delegates to [`jsonwebtoken`] using the
//!   `DecodingKey::from_jwk` adapter; this means the same claim
//!   layout (`tenant_id`, `sub`, `roles`) used by [`JwtAuthResolver`]
//!   applies here.
//!
//! Discovery and JWKS fetches use a shared [`reqwest::Client`].
//! Network errors map to `TakoError::Transport`; signature/claim
//! failures map to `TakoError::Invalid("oidc: ...")` so
//! [`crate::routes::resolve_principal`]'s 401-mapping works
//! unchanged.
//!
//! Phase 15.B.2 — RFC 7662 token introspection is now supported as an
//! opt-in post-signature-validation hook. Enable via
//! [`Self::with_introspection`] (uses the
//! `introspection_endpoint` advertised by the discovery doc; errors if
//! the issuer doesn't advertise one) or
//! [`Self::with_introspection_uri`] (explicit override). When enabled,
//! every successful signature-validated token is additionally POSTed
//! to the introspection endpoint and rejected with `TakoError::Invalid`
//! when `active=false`.
//!
//! Phase 16.B.2 — introspection now supports two
//! `introspection_endpoint_auth_method` values per RFC 7662 §2.1:
//! [`IntrospectionAuthMethod::ClientSecretBasic`] (default; Phase
//! 15.B.2 behaviour, HTTP Basic) and
//! [`IntrospectionAuthMethod::ClientSecretPost`] (credentials in the
//! form body). Choose via
//! [`OidcAuthResolver::with_introspection_auth_method`].
//!
//! Phase 17.A — discovery-driven auth-method selection. The
//! `introspection_endpoint_auth_methods_supported` field of the
//! discovery doc (RFC 8414) is now captured at construction time;
//! [`OidcAuthResolver::with_introspection_auth_method_from_discovery`]
//! picks the strongest mutually-supported method.
//!
//! Phase 17.B — [`IntrospectionAuthMethod::ClientSecretJwt`] adds
//! RFC 7521 / 7523 client-assertion JWT authentication. The resolver
//! builds a short-lived HS256 JWT signed over the configured
//! `client_secret` and sends it as the `client_assertion` form field
//! alongside `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`.
//! Auto-select prefers `client_secret_jwt` over Basic / Post when
//! the issuer advertises it AND a client_secret is configured.
//!
//! Out-of-scope (deferred to Phase 18+): `private_key_jwt`
//! (asymmetric JWT client assertions — RS256 / ES256 with separate
//! signing-key storage), refresh-token flows, end-session endpoint,
//! mTLS (`tls_client_auth` / `self_signed_tls_client_auth`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
    jwk::JwkSet,
};
use reqwest::Client;
use serde::Deserialize;
use tako_core::{Principal, TakoError};
use tokio::sync::RwLock;

use super::AuthResolver;

const DEFAULT_TENANT_CLAIM: &str = "tenant_id";
const DEFAULT_USER_CLAIM: &str = "sub";
const DEFAULT_ROLES_CLAIM: &str = "roles";
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

/// Subset of the OIDC discovery doc fields tako needs.
#[derive(Debug, Deserialize)]
struct DiscoveryDoc {
    jwks_uri: String,
    issuer: String,
    /// RFC 8414 / OIDC discovery: optional URL of the issuer's
    /// introspection endpoint (RFC 7662). When present and
    /// [`OidcAuthResolver::with_introspection`] is called, the
    /// resolver POSTs each token here for revocation-aware checks.
    #[serde(default)]
    introspection_endpoint: Option<String>,
    /// Phase 17.A — RFC 8414 list of auth methods the issuer's
    /// introspection endpoint supports. `None` (field absent) means
    /// the issuer didn't advertise; per RFC 8414 the default is
    /// `client_secret_basic`.
    #[serde(default)]
    introspection_endpoint_auth_methods_supported: Option<Vec<String>>,
    /// Phase 18.B — OIDC Session Management 1.0 §2.2.1 / 5: optional
    /// URL the relying party redirects the user-agent to in order
    /// to terminate the OP session. Captured during discovery for
    /// [`OidcAuthResolver::end_session_endpoint`] /
    /// [`OidcAuthResolver::build_logout_uri`].
    #[serde(default)]
    end_session_endpoint: Option<String>,
}

/// Phase 16.B.2 / 17.B / 18.A — RFC 7662 §2.1 introspection
/// endpoint auth method.
///
/// Selected via [`OidcAuthResolver::with_introspection_auth_method`]
/// or, in Phase 17.A, auto-negotiated against the discovery doc via
/// [`OidcAuthResolver::with_introspection_auth_method_from_discovery`].
///
/// `ClientSecretBasic` (the default) carries credentials in the
/// `Authorization: Basic ...` header; `ClientSecretPost` carries
/// them as additional fields in the form-encoded request body;
/// `ClientSecretJwt` (Phase 17.B) signs a short-lived HS256 JWT
/// over the configured `client_secret` and sends it as the
/// `client_assertion` + `client_assertion_type` form fields per
/// RFC 7521 / 7523; `PrivateKeyJwt` (Phase 18.A) does the same
/// with an asymmetric (RS256 / ES256 / EdDSA) signing key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IntrospectionAuthMethod {
    /// HTTP Basic auth — Phase 15.B.2 default behaviour.
    #[default]
    ClientSecretBasic,
    /// Credentials sent as `client_id` / `client_secret` form fields
    /// alongside `token`. Per RFC 7662 §2.1 the server MUST accept
    /// either method when authenticating a confidential client.
    ClientSecretPost,
    /// Phase 17.B — RFC 7521 / 7523 symmetric client-assertion JWT.
    ///
    /// The resolver builds a short-lived HS256 JWT signed over the
    /// configured `client_secret` (claims: `iss` / `sub` =
    /// `client_id`, `aud` = `introspect_uri`, `iat`, `exp` =
    /// `iat + 30s`, monotonic `jti`) and sends it as the
    /// `client_assertion` form field alongside
    /// `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`.
    /// No `Authorization` header is sent.
    ///
    /// Errors at request time (`introspect()`) when no
    /// `client_secret` is configured — HS256 needs the symmetric
    /// key.
    ClientSecretJwt,
    /// Phase 18.A — RFC 7521 / 7523 asymmetric client-assertion JWT.
    ///
    /// Same wire shape as [`Self::ClientSecretJwt`] (form-body
    /// `client_assertion` + `client_assertion_type`, no
    /// `Authorization` header) but signed with an RSA / EC / Ed25519
    /// private key from
    /// [`IntrospectionConfig::client_assertion_key`] instead of the
    /// symmetric `client_secret`. Algorithm selection (RS256 / ES256
    /// / EdDSA) lives on the [`ClientAssertionKey`] itself.
    ///
    /// Errors at request time when no
    /// [`IntrospectionConfig::client_assertion_key`] is configured.
    PrivateKeyJwt,
    /// Phase 24 — RFC 8705 mTLS authentication. The client presents
    /// a TLS certificate during the introspection-endpoint handshake;
    /// the issuer matches the cert's subject DN / SAN against the
    /// pre-registered `client_id`. No body credential, no
    /// `Authorization` header, no JWT.
    ///
    /// Requires [`IntrospectionConfig::mtls_client`] to be configured
    /// (a `reqwest::Client` built with
    /// [`reqwest::Identity::from_pem`]); errors at request time if
    /// missing. Configure via
    /// [`OidcAuthResolver::with_introspection_mtls`].
    TlsClientAuth,
}

/// Phase 18.A — asymmetric private signing key for the
/// [`IntrospectionAuthMethod::PrivateKeyJwt`] introspection auth
/// method. Carries the algorithm alongside the key so the signing
/// path doesn't need a separate algorithm field on
/// [`IntrospectionConfig`] (and so an HS-secret can never
/// accidentally be paired with an RS-algorithm).
///
/// Construct via the typed PEM constructors —
/// [`Self::from_rs256_pem`] / [`Self::from_es256_pem`] /
/// [`Self::from_ed25519_pem`]. The key is held in an
/// [`EncodingKey`] under the hood; [`Debug`] is implemented
/// manually so the key body is redacted.
pub struct ClientAssertionKey {
    algorithm: Algorithm,
    encoding_key: EncodingKey,
}

impl ClientAssertionKey {
    /// Load an RSA private key from a PEM encoding and pin the
    /// signing algorithm to RS256 (industry default for
    /// `private_key_jwt`).
    pub fn from_rs256_pem(pem: &[u8]) -> Result<Self, TakoError> {
        let key = EncodingKey::from_rsa_pem(pem).map_err(|e| {
            TakoError::Invalid(format!("oidc: invalid RS256 client-assertion key: {e}"))
        })?;
        Ok(Self {
            algorithm: Algorithm::RS256,
            encoding_key: key,
        })
    }

    /// Load an EC P-256 private key from a PEM encoding and pin the
    /// signing algorithm to ES256.
    pub fn from_es256_pem(pem: &[u8]) -> Result<Self, TakoError> {
        let key = EncodingKey::from_ec_pem(pem).map_err(|e| {
            TakoError::Invalid(format!("oidc: invalid ES256 client-assertion key: {e}"))
        })?;
        Ok(Self {
            algorithm: Algorithm::ES256,
            encoding_key: key,
        })
    }

    /// Load an Ed25519 private key from a PEM encoding and pin the
    /// signing algorithm to EdDSA. Newer issuers (e.g. Keycloak ≥ 22)
    /// support EdDSA client assertions.
    pub fn from_ed25519_pem(pem: &[u8]) -> Result<Self, TakoError> {
        let key = EncodingKey::from_ed_pem(pem).map_err(|e| {
            TakoError::Invalid(format!("oidc: invalid EdDSA client-assertion key: {e}"))
        })?;
        Ok(Self {
            algorithm: Algorithm::EdDSA,
            encoding_key: key,
        })
    }

    /// The pinned signing algorithm — exposed so callers can
    /// log or test against it.
    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
}

impl std::fmt::Debug for ClientAssertionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientAssertionKey")
            .field("algorithm", &self.algorithm)
            .field("encoding_key", &"<redacted>")
            .finish()
    }
}

/// Phase 15.B.2 / 18.A — RFC 7662 token-introspection configuration.
///
/// When attached to an [`OidcAuthResolver`] via
/// [`OidcAuthResolver::with_introspection`] or
/// [`OidcAuthResolver::with_introspection_uri`], every signature-
/// validated token is additionally POSTed to `introspect_uri` with
/// the token in the `token` form field and `client_id` /
/// `client_secret` carried per the configured
/// [`IntrospectionAuthMethod`] (Phase 16.B.2; defaults to HTTP
/// Basic). A response with `active=false` rejects the token with
/// `TakoError::Invalid("oidc: token revoked (introspection)")`.
#[derive(Debug, Clone)]
pub struct IntrospectionConfig {
    pub introspect_uri: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    /// Phase 16.B.2 — RFC 7662 §2.1 introspection auth method.
    /// Defaults to `ClientSecretBasic` (Phase 15.B.2 behaviour).
    pub auth_method: IntrospectionAuthMethod,
    /// Phase 18.A — asymmetric signing key for
    /// [`IntrospectionAuthMethod::PrivateKeyJwt`]. `Arc` because
    /// `EncodingKey` doesn't impl `Clone` and
    /// [`OidcAuthResolver`] is `#[derive(Clone)]` for the Python
    /// immutable-builder pattern. `None` for symmetric methods.
    pub client_assertion_key: Option<Arc<ClientAssertionKey>>,
    /// Phase 24 — mTLS-enabled HTTP client for
    /// [`IntrospectionAuthMethod::TlsClientAuth`]. Built eagerly at
    /// builder time (`with_introspection_mtls`) so PEM parsing
    /// failures surface as `TakoError::Invalid` early rather than
    /// at first-request time. `Arc<reqwest::Client>` because
    /// `Client` is already internally `Arc`'d; cloning is cheap.
    /// `None` for non-mTLS auth methods.
    pub mtls_client: Option<Arc<reqwest::Client>>,
}

/// Subset of RFC 7662 introspection response. `active` is the only
/// field we act on; others are ignored.
#[derive(Debug, Deserialize)]
struct IntrospectionResponse {
    #[serde(default)]
    active: bool,
}

/// JWKS fetched from `jwks_uri`. Validation hint: a missing-`kid`
/// failure triggers a single force-refresh and retry.
#[derive(Debug)]
struct CachedJwks {
    jwks: JwkSet,
    fetched_at: Instant,
}

#[derive(Clone)]
pub struct OidcAuthResolver {
    issuer: String,
    audience: String,
    jwks_uri: String,
    http: Client,
    cache: Arc<RwLock<Option<CachedJwks>>>,
    refresh_interval: Duration,
    tenant_claim: String,
    user_claim: String,
    roles_claim: String,
    /// Phase 15.B.2 — `introspection_endpoint` advertised by the
    /// issuer's discovery doc, captured for use by
    /// [`Self::with_introspection`].
    discovered_introspection_uri: Option<String>,
    /// Phase 17.A — RFC 8414
    /// `introspection_endpoint_auth_methods_supported` advertised by
    /// the issuer's discovery doc. `None` means the field was absent
    /// (RFC 8414: default is `client_secret_basic`); `Some(vec![])`
    /// means the issuer explicitly advertised an empty list.
    discovered_introspection_auth_methods: Option<Vec<String>>,
    /// Phase 18.B — OIDC Session Management 1.0 `end_session_endpoint`
    /// captured at discovery time. `None` means the issuer doesn't
    /// implement OIDC Session Management.
    discovered_end_session_uri: Option<String>,
    /// Phase 15.B.2 — when `Some`, every signature-validated token is
    /// additionally POSTed for an `active=true` check.
    introspection: Option<IntrospectionConfig>,
}

impl std::fmt::Debug for OidcAuthResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuthResolver")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("jwks_uri", &self.jwks_uri)
            .field("refresh_interval", &self.refresh_interval)
            .finish_non_exhaustive()
    }
}

impl OidcAuthResolver {
    /// Discover an OIDC provider at `issuer` and configure the
    /// resolver to require `audience` (`aud` claim) on incoming
    /// tokens.
    pub async fn discover(issuer: &str, audience: &str) -> Result<Self, TakoError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| TakoError::Transport(format!("oidc: failed to build http client: {e}")))?;
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let resp = http
            .get(&url)
            .send()
            .await
            .map_err(|e| TakoError::Transport(format!("oidc: discovery GET {url} failed: {e}")))?
            .error_for_status()
            .map_err(|e| TakoError::Transport(format!("oidc: discovery {url}: {e}")))?;
        let doc: DiscoveryDoc = resp
            .json()
            .await
            .map_err(|e| TakoError::Invalid(format!("oidc: malformed discovery doc: {e}")))?;
        if doc.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
            return Err(TakoError::Invalid(format!(
                "oidc: discovery `issuer` ({}) does not match configured issuer ({})",
                doc.issuer, issuer,
            )));
        }
        Ok(Self {
            issuer: issuer.into(),
            audience: audience.into(),
            jwks_uri: doc.jwks_uri,
            http,
            cache: Arc::new(RwLock::new(None)),
            refresh_interval: DEFAULT_REFRESH_INTERVAL,
            tenant_claim: DEFAULT_TENANT_CLAIM.into(),
            user_claim: DEFAULT_USER_CLAIM.into(),
            roles_claim: DEFAULT_ROLES_CLAIM.into(),
            discovered_introspection_uri: doc.introspection_endpoint,
            discovered_introspection_auth_methods: doc
                .introspection_endpoint_auth_methods_supported,
            discovered_end_session_uri: doc.end_session_endpoint,
            introspection: None,
        })
    }

    /// Override the lazy JWKS-refresh interval (default 1h).
    pub fn with_refresh_interval(mut self, d: Duration) -> Self {
        self.refresh_interval = d;
        self
    }

    /// Override the claim names that map to `Principal` fields.
    pub fn with_claims(mut self, tenant: &str, user: &str, roles: &str) -> Self {
        self.tenant_claim = tenant.into();
        self.user_claim = user.into();
        self.roles_claim = roles.into();
        self
    }

    /// Phase 15.B.2 — enable RFC 7662 token introspection using the
    /// `introspection_endpoint` advertised by the issuer's discovery
    /// doc.
    ///
    /// Errors if the discovery doc did not advertise
    /// `introspection_endpoint`. Use
    /// [`Self::with_introspection_uri`] to bypass discovery and supply
    /// an explicit URL.
    ///
    /// `client_secret` is sent over HTTP Basic auth alongside
    /// `client_id`. Pass `None` for public clients (rare).
    pub fn with_introspection(
        mut self,
        client_id: impl Into<String>,
        client_secret: Option<String>,
    ) -> Result<Self, TakoError> {
        let uri = self.discovered_introspection_uri.clone().ok_or_else(|| {
            TakoError::Invalid(
                "oidc: issuer did not advertise `introspection_endpoint`; \
                 use `with_introspection_uri` for explicit URI"
                    .into(),
            )
        })?;
        self.introspection = Some(IntrospectionConfig {
            introspect_uri: uri,
            client_id: client_id.into(),
            client_secret,
            auth_method: IntrospectionAuthMethod::default(),
            client_assertion_key: None,
            mtls_client: None,
        });
        Ok(self)
    }

    /// Phase 15.B.2 — enable RFC 7662 token introspection with an
    /// explicit endpoint URL (bypasses discovery). Infallible.
    pub fn with_introspection_uri(
        mut self,
        uri: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: Option<String>,
    ) -> Self {
        self.introspection = Some(IntrospectionConfig {
            introspect_uri: uri.into(),
            client_id: client_id.into(),
            client_secret,
            auth_method: IntrospectionAuthMethod::default(),
            client_assertion_key: None,
            mtls_client: None,
        });
        self
    }

    /// Phase 16.B.2 — override the
    /// [`IntrospectionAuthMethod`] used to authenticate
    /// introspection requests. Chainable on top of
    /// [`Self::with_introspection`] or
    /// [`Self::with_introspection_uri`]; no-op (and silently
    /// returned unchanged) when no introspection config has been
    /// attached yet.
    pub fn with_introspection_auth_method(mut self, method: IntrospectionAuthMethod) -> Self {
        if let Some(cfg) = self.introspection.as_mut() {
            cfg.auth_method = method;
        }
        self
    }

    /// Phase 17.A / 17.B / 18.A / 24 — auto-select the
    /// [`IntrospectionAuthMethod`] from the issuer's RFC 8414
    /// `introspection_endpoint_auth_methods_supported` list captured
    /// during discovery. Preference order (strongest first):
    /// `tls_client_auth` (Phase 24; only when an mTLS identity is
    /// configured) → `private_key_jwt` (Phase 18.A; only when a
    /// `client_assertion_key` is configured) → `client_secret_jwt`
    /// (Phase 17.B; only when a `client_secret` is configured —
    /// HS256 needs the symmetric key) → `client_secret_basic` →
    /// `client_secret_post`.
    ///
    /// Behaviour:
    /// - Silent no-op (returns `Ok(self)` unchanged) when no
    ///   introspection config has been attached yet — matches the
    ///   chainable-builder cadence of
    ///   [`Self::with_introspection_auth_method`].
    /// - When discovery did not advertise the field (`None`):
    ///   selects `ClientSecretBasic` per RFC 8414's documented
    ///   default.
    /// - When discovery advertised a list with at least one
    ///   supported variant: selects the strongest (preference
    ///   order above).
    /// - When discovery advertised a list with **no** supported
    ///   variant (e.g. issuer requires only
    ///   `self_signed_tls_client_auth`, deferred to Phase 25+):
    ///   returns [`TakoError::Invalid`] so the operator notices at
    ///   builder time rather than at HTTP-401 from the
    ///   introspection endpoint.
    pub fn with_introspection_auth_method_from_discovery(mut self) -> Result<Self, TakoError> {
        let Some(cfg) = self.introspection.as_mut() else {
            return Ok(self);
        };
        let advertised = self.discovered_introspection_auth_methods.as_deref();
        let picked = match advertised {
            None => IntrospectionAuthMethod::ClientSecretBasic,
            Some(list) => {
                let has_secret = cfg.client_secret.is_some();
                let has_key = cfg.client_assertion_key.is_some();
                let has_mtls = cfg.mtls_client.is_some();
                let supports = |needle: &str| list.iter().any(|m| m == needle);
                if has_mtls && supports("tls_client_auth") {
                    IntrospectionAuthMethod::TlsClientAuth
                } else if has_key && supports("private_key_jwt") {
                    IntrospectionAuthMethod::PrivateKeyJwt
                } else if has_secret && supports("client_secret_jwt") {
                    IntrospectionAuthMethod::ClientSecretJwt
                } else if supports("client_secret_basic") {
                    IntrospectionAuthMethod::ClientSecretBasic
                } else if supports("client_secret_post") {
                    IntrospectionAuthMethod::ClientSecretPost
                } else {
                    return Err(TakoError::Invalid(format!(
                        "oidc: no supported introspection auth method advertised \
                         by issuer; supported: {list:?}"
                    )));
                }
            }
        };
        cfg.auth_method = picked;
        Ok(self)
    }

    /// Phase 18.A — attach an asymmetric [`ClientAssertionKey`] for
    /// the [`IntrospectionAuthMethod::PrivateKeyJwt`] auth method.
    /// Chainable on top of [`Self::with_introspection`] /
    /// [`Self::with_introspection_uri`]; silent no-op when no
    /// introspection config has been attached yet.
    ///
    /// Does **not** also flip the auth method — call
    /// [`Self::with_introspection_auth_method`] (with
    /// `IntrospectionAuthMethod::PrivateKeyJwt`) afterwards, or use
    /// [`Self::with_introspection_auth_method_from_discovery`]
    /// to auto-negotiate based on the issuer's advertised list.
    pub fn with_introspection_private_key(mut self, key: ClientAssertionKey) -> Self {
        if let Some(cfg) = self.introspection.as_mut() {
            cfg.client_assertion_key = Some(Arc::new(key));
        }
        self
    }

    /// Phase 18.A — convenience: load an RS256 PEM, attach it as
    /// the [`ClientAssertionKey`], AND flip the auth method to
    /// [`IntrospectionAuthMethod::PrivateKeyJwt`].
    pub fn with_introspection_jwt_rs256_pem(mut self, pem: &[u8]) -> Result<Self, TakoError> {
        let key = ClientAssertionKey::from_rs256_pem(pem)?;
        if let Some(cfg) = self.introspection.as_mut() {
            cfg.client_assertion_key = Some(Arc::new(key));
            cfg.auth_method = IntrospectionAuthMethod::PrivateKeyJwt;
        }
        Ok(self)
    }

    /// Phase 18.A — ES256 sibling of
    /// [`Self::with_introspection_jwt_rs256_pem`].
    pub fn with_introspection_jwt_es256_pem(mut self, pem: &[u8]) -> Result<Self, TakoError> {
        let key = ClientAssertionKey::from_es256_pem(pem)?;
        if let Some(cfg) = self.introspection.as_mut() {
            cfg.client_assertion_key = Some(Arc::new(key));
            cfg.auth_method = IntrospectionAuthMethod::PrivateKeyJwt;
        }
        Ok(self)
    }

    /// Phase 18.A — EdDSA sibling of
    /// [`Self::with_introspection_jwt_rs256_pem`].
    pub fn with_introspection_jwt_ed25519_pem(mut self, pem: &[u8]) -> Result<Self, TakoError> {
        let key = ClientAssertionKey::from_ed25519_pem(pem)?;
        if let Some(cfg) = self.introspection.as_mut() {
            cfg.client_assertion_key = Some(Arc::new(key));
            cfg.auth_method = IntrospectionAuthMethod::PrivateKeyJwt;
        }
        Ok(self)
    }

    /// Phase 24 — load a client cert + private key from separate
    /// PEM blobs, build an mTLS-enabled [`reqwest::Client`], and
    /// switch the introspection auth method to
    /// [`IntrospectionAuthMethod::TlsClientAuth`]. PEM parse failure
    /// (or `reqwest::Client` build failure) surfaces as
    /// [`TakoError::Invalid`] at builder time so operators notice
    /// before the first request.
    ///
    /// `cert_pem` should be a PEM-encoded X.509 certificate (or
    /// chain — [`reqwest::Identity::from_pem`] accepts concatenated
    /// certs). `key_pem` should be a PKCS#8 or SEC1-encoded private
    /// key matching the cert.
    ///
    /// Silent no-op (returns `Ok(self)` unchanged) when no
    /// introspection config has been attached yet — matches the
    /// chainable-builder cadence of the other
    /// `with_introspection_*` builders.
    pub fn with_introspection_mtls(
        mut self,
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> Result<Self, TakoError> {
        // Silent no-op when no introspection config is attached —
        // skip the cert / Client build to avoid surfacing PEM
        // errors that the operator will never use.
        if self.introspection.is_none() {
            return Ok(self);
        }
        let identity = build_mtls_identity(cert_pem, key_pem)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .identity(identity)
            .build()
            .map_err(|e| TakoError::Invalid(format!("oidc: failed to build mTLS client: {e}")))?;
        if let Some(cfg) = self.introspection.as_mut() {
            cfg.mtls_client = Some(Arc::new(client));
            cfg.auth_method = IntrospectionAuthMethod::TlsClientAuth;
        }
        Ok(self)
    }

    /// Phase 24 — convenience for combined PEM blobs (the common
    /// output format from `cat cert.pem key.pem`). Delegates to
    /// [`Self::with_introspection_mtls`] with the same blob passed
    /// twice; [`reqwest::Identity::from_pem`] locates the cert and
    /// key blocks by PEM section markers independently.
    pub fn with_introspection_mtls_combined(self, combined_pem: &[u8]) -> Result<Self, TakoError> {
        self.with_introspection_mtls(combined_pem, combined_pem)
    }

    /// Phase 18.B — return the OIDC Session Management 1.0
    /// `end_session_endpoint` URL the issuer advertised at discovery
    /// time. `None` when the issuer doesn't implement OIDC Session
    /// Management.
    pub fn end_session_endpoint(&self) -> Option<&str> {
        self.discovered_end_session_uri.as_deref()
    }

    /// Phase 18.B — build a logout URL per OIDC Session Management
    /// 1.0 §5. Returns `None` when the issuer didn't advertise
    /// `end_session_endpoint` (the most common case for OIDC
    /// providers that don't ship Session Management).
    ///
    /// All query parameters are optional; passing `None` for
    /// everything yields the bare endpoint URL. Spec parameters
    /// honoured:
    ///
    /// - `id_token_hint` — the ID token whose subject the relying
    ///   party wants logged out. RECOMMENDED by the spec.
    /// - `post_logout_redirect_uri` — where the OP redirects the
    ///   user-agent after logout completes. Must be pre-registered
    ///   with the OP.
    /// - `state` — round-tripped opaque value for CSRF mitigation.
    ///
    /// When the configured `end_session_endpoint` already carries
    /// a query string, the new params are appended via
    /// [`url::form_urlencoded`] using the same separator semantics
    /// reqwest uses internally.
    pub fn build_logout_uri(
        &self,
        id_token_hint: Option<&str>,
        post_logout_redirect_uri: Option<&str>,
        state: Option<&str>,
    ) -> Option<String> {
        let base = self.discovered_end_session_uri.as_deref()?;
        // Build the query-string fragment in a tight scope —
        // `form_urlencoded::Serializer` is not `Send`, but this fn
        // is sync so that's not an issue here.
        let qs = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            let mut any = false;
            if let Some(hint) = id_token_hint {
                form.append_pair("id_token_hint", hint);
                any = true;
            }
            if let Some(uri) = post_logout_redirect_uri {
                form.append_pair("post_logout_redirect_uri", uri);
                any = true;
            }
            if let Some(s) = state {
                form.append_pair("state", s);
                any = true;
            }
            if any { Some(form.finish()) } else { None }
        };
        let Some(qs) = qs else {
            return Some(base.to_string());
        };
        // Append with `?` or `&` depending on whether the configured
        // endpoint already has a query string. RFC 3986 reserves
        // `?` for the start of the query component.
        let sep = if base.contains('?') { '&' } else { '?' };
        Some(format!("{base}{sep}{qs}"))
    }

    /// Phase 15.B.2 / 17.B — POST the token to the introspection
    /// endpoint and confirm `active=true`. Returns `Err` with a
    /// `TakoError::Invalid("oidc: token revoked ...")` payload when
    /// the issuer rejects the token.
    async fn introspect(&self, token: &str) -> Result<(), TakoError> {
        let Some(cfg) = &self.introspection else {
            return Ok(());
        };
        // Phase 17.B / 18.A — `ClientSecretJwt` requires a non-`None`
        // `client_secret` (HS256 over symmetric key);
        // `PrivateKeyJwt` requires a `client_assertion_key`
        // (asymmetric RS256 / ES256 / EdDSA). Build the assertion
        // before the form-body scope so the encoding
        // `Result<String, _>` short-circuits cleanly.
        let assertion: Option<String> = match cfg.auth_method {
            IntrospectionAuthMethod::ClientSecretJwt => {
                let secret = cfg.client_secret.as_deref().ok_or_else(|| {
                    TakoError::Invalid(
                        "oidc: client_secret_jwt requires client_secret to be set".into(),
                    )
                })?;
                Some(build_client_assertion(
                    &cfg.client_id,
                    &cfg.introspect_uri,
                    &EncodingKey::from_secret(secret.as_bytes()),
                    Algorithm::HS256,
                )?)
            }
            IntrospectionAuthMethod::PrivateKeyJwt => {
                let key = cfg.client_assertion_key.as_deref().ok_or_else(|| {
                    TakoError::Invalid(
                        "oidc: private_key_jwt requires client_assertion_key to be set".into(),
                    )
                })?;
                Some(build_client_assertion(
                    &cfg.client_id,
                    &cfg.introspect_uri,
                    &key.encoding_key,
                    key.algorithm,
                )?)
            }
            _ => None,
        };

        // Phase 24 — `TlsClientAuth` requires a non-`None`
        // `mtls_client` (the per-resolver mTLS-enabled HTTP client
        // built at builder time by `with_introspection_mtls`).
        if cfg.auth_method == IntrospectionAuthMethod::TlsClientAuth && cfg.mtls_client.is_none() {
            return Err(TakoError::Invalid(
                "oidc: tls_client_auth requires mtls_client to be set".into(),
            ));
        }

        // Workspace reqwest is configured without the `urlencoded`
        // feature; build the form body manually via `url`'s
        // `form_urlencoded`. This is what reqwest's `.form()` does
        // internally.
        //
        // Phase 16.B.2 / 17.B / 18.A / 24 — credential carriage:
        // - `ClientSecretBasic`: body credential-free; `Authorization:
        //   Basic` header added below.
        // - `ClientSecretPost`: `client_id` / `client_secret` in body.
        // - `ClientSecretJwt` / `PrivateKeyJwt`: `client_assertion` /
        //   `client_assertion_type` in body, no Authorization
        //   header.
        // - `TlsClientAuth`: body credential-free, no Authorization
        //   header. The issuer authenticates via the TLS handshake
        //   cert; tako swaps in the mTLS-enabled `reqwest::Client`
        //   below.
        //
        // The `Serializer` is not `Send`, so build the body string
        // in a tight scope that drops it before any await.
        let body = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            form.append_pair("token", token)
                .append_pair("token_type_hint", "access_token");
            match cfg.auth_method {
                IntrospectionAuthMethod::ClientSecretBasic => {}
                IntrospectionAuthMethod::ClientSecretPost => {
                    form.append_pair("client_id", &cfg.client_id);
                    if let Some(secret) = &cfg.client_secret {
                        form.append_pair("client_secret", secret);
                    }
                }
                IntrospectionAuthMethod::ClientSecretJwt
                | IntrospectionAuthMethod::PrivateKeyJwt => {
                    // RFC 7521 §4.2 — fixed type URI plus the
                    // assertion JWT we built above.
                    form.append_pair(
                        "client_assertion_type",
                        "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                    );
                    if let Some(jwt) = assertion.as_deref() {
                        form.append_pair("client_assertion", jwt);
                    }
                }
                IntrospectionAuthMethod::TlsClientAuth => {}
            }
            form.finish()
        };
        // Phase 24 — for `TlsClientAuth`, swap to the mTLS-enabled
        // `reqwest::Client` cached on the config; other auth
        // methods use the resolver's default HTTP client.
        let http: &reqwest::Client = match cfg.auth_method {
            IntrospectionAuthMethod::TlsClientAuth => cfg
                .mtls_client
                .as_deref()
                // Guarded above; safe.
                .unwrap_or(&self.http),
            _ => &self.http,
        };
        let req = http
            .post(&cfg.introspect_uri)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body);
        let req = match cfg.auth_method {
            IntrospectionAuthMethod::ClientSecretBasic => {
                if let Some(secret) = &cfg.client_secret {
                    req.basic_auth(&cfg.client_id, Some(secret))
                } else {
                    req.basic_auth(&cfg.client_id, None::<&str>)
                }
            }
            // Phase 16.B.2 / 17.B / 18.A / 24 — Post, JWT, and mTLS
            // variants all skip the Authorization header.
            IntrospectionAuthMethod::ClientSecretPost
            | IntrospectionAuthMethod::ClientSecretJwt
            | IntrospectionAuthMethod::PrivateKeyJwt
            | IntrospectionAuthMethod::TlsClientAuth => req,
        };
        let resp = req.send().await.map_err(|e| {
            TakoError::Transport(format!("oidc: introspect POST {}: {e}", cfg.introspect_uri))
        })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(TakoError::Invalid(format!(
                "oidc: introspect endpoint returned {status}: {body}"
            )));
        }
        let parsed: IntrospectionResponse = resp
            .json()
            .await
            .map_err(|e| TakoError::Invalid(format!("oidc: introspect response parse: {e}")))?;
        if !parsed.active {
            return Err(TakoError::Invalid(
                "oidc: token revoked (introspection `active=false`)".into(),
            ));
        }
        Ok(())
    }

    /// Returns a JWKS guaranteed to be no older than `refresh_interval`.
    /// Force-refreshes if the cache is empty or stale.
    async fn jwks(&self, force: bool) -> Result<JwkSet, TakoError> {
        if !force {
            let guard = self.cache.read().await;
            if let Some(c) = guard.as_ref() {
                if c.fetched_at.elapsed() < self.refresh_interval {
                    return Ok(c.jwks.clone());
                }
            }
        }
        // Drop read guard, take write lock, double-check, fetch.
        let mut guard = self.cache.write().await;
        if !force {
            if let Some(c) = guard.as_ref() {
                if c.fetched_at.elapsed() < self.refresh_interval {
                    return Ok(c.jwks.clone());
                }
            }
        }
        let resp = self
            .http
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|e| {
                TakoError::Transport(format!("oidc: jwks GET {} failed: {e}", self.jwks_uri))
            })?
            .error_for_status()
            .map_err(|e| TakoError::Transport(format!("oidc: jwks {}: {e}", self.jwks_uri)))?;
        let jwks: JwkSet = resp
            .json()
            .await
            .map_err(|e| TakoError::Invalid(format!("oidc: malformed JWKS: {e}")))?;
        *guard = Some(CachedJwks {
            jwks: jwks.clone(),
            fetched_at: Instant::now(),
        });
        Ok(jwks)
    }

    fn principal_from_claims(
        &self,
        claims: &BTreeMap<String, serde_json::Value>,
    ) -> Result<Principal, TakoError> {
        let tenant_id = claims
            .get(&self.tenant_claim)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                TakoError::Invalid(format!(
                    "oidc: missing or non-string claim `{}`",
                    self.tenant_claim
                ))
            })?
            .to_string();
        let user_id = claims
            .get(&self.user_claim)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                TakoError::Invalid(format!(
                    "oidc: missing or non-string claim `{}`",
                    self.user_claim
                ))
            })?
            .to_string();
        let roles = claims
            .get(&self.roles_claim)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(Principal {
            tenant_id,
            user_id,
            roles,
            trace_id: None,
            metadata: Default::default(),
        })
    }

    async fn validate_against(&self, token: &str, jwks: &JwkSet) -> Result<Principal, TakoError> {
        let header = decode_header(token)
            .map_err(|e| TakoError::Invalid(format!("oidc: malformed JWT header: {e}")))?;
        let kid = header
            .kid
            .as_ref()
            .ok_or_else(|| TakoError::Invalid("oidc: JWT missing `kid` header".into()))?;
        let jwk = jwks.find(kid).ok_or_else(|| {
            TakoError::Invalid(format!("oidc: no JWK in cache matches `kid` = {kid}"))
        })?;
        let alg = match header.alg {
            Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::ES256
            | Algorithm::ES384
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512
            | Algorithm::EdDSA => header.alg,
            other => {
                return Err(TakoError::Invalid(format!(
                    "oidc: rejecting unsupported alg `{other:?}` (HS* and `none` not allowed)"
                )));
            }
        };
        let mut validation = Validation::new(alg);
        validation.set_audience(std::slice::from_ref(&self.audience));
        validation.set_issuer(std::slice::from_ref(&self.issuer));
        validation.required_spec_claims.clear();
        validation.required_spec_claims.insert("exp".into());
        validation.required_spec_claims.insert("iss".into());
        validation.required_spec_claims.insert("aud".into());

        let key = DecodingKey::from_jwk(jwk)
            .map_err(|e| TakoError::Invalid(format!("oidc: cannot build DecodingKey: {e}")))?;
        let data = decode::<BTreeMap<String, serde_json::Value>>(token, &key, &validation)
            .map_err(|e| TakoError::Invalid(format!("oidc: {e}")))?;
        self.principal_from_claims(&data.claims)
    }
}

// -------------------------------------------------------------------------
// Phase 17.B — RFC 7521 / 7523 client-assertion JWT builder.
// -------------------------------------------------------------------------

/// Process-monotonic JTI counter — pairs with a wall-clock nanosecond
/// component so the resulting `jti` is unique within an issuer's
/// 30-second assertion-validity window with effectively zero collision
/// risk, even across process restarts. RFC 7519 §4.1.7 only requires
/// uniqueness within the issuer's tokens; that bar is easily met.
static JTI_COUNTER: AtomicU64 = AtomicU64::new(0);

fn make_jti() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let ctr = JTI_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{ctr:x}")
}

/// Phase 17.B / 18.A — build a short-lived client-assertion JWT for
/// RFC 7521 / 7523 (`client_secret_jwt` or `private_key_jwt`)
/// introspection auth. Claims:
/// - `iss` = `sub` = `client_id`
/// - `aud` = `audience` (the introspection endpoint URI per RFC 7523
///   §3 — the assertion is bound to its target endpoint to prevent
///   replay against a different endpoint at the same authorization
///   server).
/// - `iat` = unix-now
/// - `exp` = `iat + 30s` (RFC 7521 §4.2 recommends a "short lifetime")
/// - `jti` = monotonic per-call identifier from [`make_jti`]
///
/// The signing algorithm + key are passed in: HS256 over a symmetric
/// `client_secret` (Phase 17.B `client_secret_jwt`) or RS256 / ES256
/// / EdDSA over an asymmetric private key (Phase 18.A
/// `private_key_jwt`).
fn build_client_assertion(
    client_id: &str,
    audience: &str,
    encoding_key: &EncodingKey,
    algorithm: Algorithm,
) -> Result<String, TakoError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| TakoError::Invalid(format!("oidc: clock before unix epoch: {e}")))?
        .as_secs();
    let claims = serde_json::json!({
        "iss": client_id,
        "sub": client_id,
        "aud": audience,
        "iat": now,
        "exp": now + 30,
        "jti": make_jti(),
    });
    encode(&Header::new(algorithm), &claims, encoding_key).map_err(|e| {
        TakoError::Invalid(format!("oidc: client-assertion sign ({algorithm:?}): {e}"))
    })
}

/// Phase 24 — build a [`reqwest::Identity`] from separate cert +
/// key PEM blobs. `reqwest::Identity::from_pem` requires both
/// pieces in a single concatenated blob; this helper handles the
/// concatenation (preserving a separating newline).
///
/// Errors map to `TakoError::Invalid("oidc: invalid mTLS identity
/// PEM: ...")` so the operator gets a clear diagnostic when their
/// cert or key file is malformed.
fn build_mtls_identity(cert_pem: &[u8], key_pem: &[u8]) -> Result<reqwest::Identity, TakoError> {
    let mut combined = Vec::with_capacity(cert_pem.len() + key_pem.len() + 1);
    combined.extend_from_slice(cert_pem);
    if !cert_pem.ends_with(b"\n") {
        combined.push(b'\n');
    }
    combined.extend_from_slice(key_pem);
    reqwest::Identity::from_pem(&combined)
        .map_err(|e| TakoError::Invalid(format!("oidc: invalid mTLS identity PEM: {e}")))
}

#[async_trait]
impl AuthResolver for OidcAuthResolver {
    async fn resolve(&self, token: &str) -> Result<Principal, TakoError> {
        // First try the cached JWKS; if validation fails because of a
        // missing `kid` or signature mismatch, force-refresh once and
        // retry. The retry handles the documented JWKS-rotation race.
        let jwks = self.jwks(false).await?;
        let principal = match self.validate_against(token, &jwks).await {
            Ok(p) => p,
            Err(TakoError::Invalid(msg))
                if msg.contains("InvalidSignature")
                    || msg.contains("no JWK in cache matches")
                    || msg.contains("InvalidKid") =>
            {
                let fresh = self.jwks(true).await?;
                self.validate_against(token, &fresh).await?
            }
            Err(other) => return Err(other),
        };

        // Phase 15.B.2 — when introspection is configured, fail-closed
        // on `active=false`. No-op when introspection is not enabled.
        self.introspect(token).await?;

        Ok(principal)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use jsonwebtoken::jwk::JwkSet;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    /// Construct a minimal `OidcAuthResolver` for testing without
    /// hitting a live OIDC discovery endpoint.
    fn test_resolver(http: Client, issuer: &str) -> OidcAuthResolver {
        OidcAuthResolver {
            issuer: issuer.into(),
            audience: "test-audience".into(),
            jwks_uri: format!("{issuer}/jwks"),
            http,
            cache: Arc::new(RwLock::new(Some(CachedJwks {
                jwks: JwkSet { keys: vec![] },
                fetched_at: Instant::now(),
            }))),
            refresh_interval: DEFAULT_REFRESH_INTERVAL,
            tenant_claim: DEFAULT_TENANT_CLAIM.into(),
            user_claim: DEFAULT_USER_CLAIM.into(),
            roles_claim: DEFAULT_ROLES_CLAIM.into(),
            discovered_introspection_uri: None,
            discovered_introspection_auth_methods: None,
            discovered_end_session_uri: None,
            introspection: None,
        }
    }

    #[test]
    fn oidc_resolver_is_send_sync() {
        assert_send_sync::<OidcAuthResolver>();
    }

    #[test]
    fn introspection_config_is_clone_debug() {
        let cfg = IntrospectionConfig {
            introspect_uri: "https://issuer/introspect".into(),
            client_id: "id".into(),
            client_secret: Some("secret".into()),
            auth_method: IntrospectionAuthMethod::default(),
            client_assertion_key: None,
            mtls_client: None,
        };
        let cloned = cfg.clone();
        assert_eq!(cloned.introspect_uri, cfg.introspect_uri);
        assert_eq!(
            cloned.auth_method,
            IntrospectionAuthMethod::ClientSecretBasic
        );
        let _ = format!("{cfg:?}");
    }

    #[test]
    fn with_introspection_errors_when_no_endpoint_advertised() {
        let http = Client::new();
        let r = test_resolver(http, "https://issuer.example");
        // No `introspection_endpoint` was discovered — `with_introspection`
        // must fail-closed.
        let err = r.with_introspection("client-id", None).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("introspection_endpoint"),
            "expected fail-closed error, got: {msg}"
        );
    }

    #[test]
    fn with_introspection_succeeds_when_discovery_advertised_endpoint() {
        let http = Client::new();
        let mut r = test_resolver(http, "https://issuer.example");
        r.discovered_introspection_uri = Some("https://issuer.example/introspect".into());
        let r = r
            .with_introspection("client-id", Some("secret".into()))
            .unwrap();
        let cfg = r.introspection.expect("introspection set");
        assert_eq!(cfg.introspect_uri, "https://issuer.example/introspect");
        assert_eq!(cfg.client_id, "client-id");
        assert_eq!(cfg.client_secret.as_deref(), Some("secret"));
    }

    #[test]
    fn with_introspection_uri_bypasses_discovery() {
        let http = Client::new();
        let r = test_resolver(http, "https://issuer.example");
        // No `introspection_endpoint` from discovery — explicit URI
        // must still work.
        let r = r.with_introspection_uri("https://override/introspect", "client-id", None);
        let cfg = r.introspection.expect("introspection set");
        assert_eq!(cfg.introspect_uri, "https://override/introspect");
        assert!(cfg.client_secret.is_none());
    }

    #[tokio::test]
    async fn introspect_active_true_returns_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .expect(1)
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example").with_introspection_uri(
            format!("{}/introspect", server.uri()),
            "client",
            Some("secret".into()),
        );
        r.introspect("any-token").await.unwrap();
    }

    #[tokio::test]
    async fn introspect_active_false_returns_invalid_revoked() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": false })))
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example").with_introspection_uri(
            format!("{}/introspect", server.uri()),
            "client",
            None,
        );
        let err = r.introspect("any-token").await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("revoked"), "got: {msg}");
        assert!(msg.contains("introspection"), "got: {msg}");
    }

    #[tokio::test]
    async fn introspect_carries_basic_auth_with_secret() {
        let server = MockServer::start().await;
        // Authorization: Basic base64("client:secret") =
        //   "Basic Y2xpZW50OnNlY3JldA=="
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .and(header("Authorization", "Basic Y2xpZW50OnNlY3JldA=="))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .expect(1)
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example").with_introspection_uri(
            format!("{}/introspect", server.uri()),
            "client",
            Some("secret".into()),
        );
        r.introspect("any-token").await.unwrap();
    }

    #[tokio::test]
    async fn introspect_propagates_5xx_as_invalid() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example").with_introspection_uri(
            format!("{}/introspect", server.uri()),
            "client",
            None,
        );
        let err = r.introspect("any-token").await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("500"), "got: {msg}");
    }

    #[tokio::test]
    async fn introspect_no_op_when_disabled() {
        // `introspection.is_none()` — `introspect()` must succeed
        // without making any HTTP call.
        let r = test_resolver(Client::new(), "https://issuer.example");
        r.introspect("any-token").await.unwrap();
    }

    // -----------------------------------------------------------------
    // Phase 16.B.2 — `IntrospectionAuthMethod::ClientSecretPost`.
    // -----------------------------------------------------------------

    #[test]
    fn introspection_auth_method_default_is_basic() {
        assert_eq!(
            IntrospectionAuthMethod::default(),
            IntrospectionAuthMethod::ClientSecretBasic
        );
    }

    #[test]
    fn with_introspection_auth_method_overrides_default() {
        let http = Client::new();
        let r = test_resolver(http, "https://issuer.example")
            .with_introspection_uri("https://override/introspect", "client-id", None)
            .with_introspection_auth_method(IntrospectionAuthMethod::ClientSecretPost);
        let cfg = r.introspection.expect("introspection set");
        assert_eq!(cfg.auth_method, IntrospectionAuthMethod::ClientSecretPost);
    }

    #[test]
    fn with_introspection_auth_method_no_op_without_introspection_config() {
        // No introspection attached yet — call is a silent no-op rather
        // than a panic.
        let http = Client::new();
        let r = test_resolver(http, "https://issuer.example")
            .with_introspection_auth_method(IntrospectionAuthMethod::ClientSecretPost);
        assert!(r.introspection.is_none());
    }

    #[tokio::test]
    async fn introspect_post_carries_credentials_in_form_body() {
        // Phase 16.B.2 — `ClientSecretPost` must NOT send
        // `Authorization: Basic`, and MUST include `client_id` /
        // `client_secret` form fields alongside `token`.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/introspect"))
            // RFC 7662 §2.1 ClientSecretPost: credentials in body.
            .and(wiremock::matchers::body_string_contains("client_id=client"))
            .and(wiremock::matchers::body_string_contains(
                "client_secret=topsecret",
            ))
            .and(wiremock::matchers::body_string_contains("token=abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .expect(1)
            .mount(&server)
            .await;
        // Catch-all — any request that included an Authorization
        // header would NOT match the above (no Authorization matcher)
        // and would fall through to a 404, which the assertion below
        // would surface as a failure.

        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_uri(
                format!("{}/introspect", server.uri()),
                "client",
                Some("topsecret".into()),
            )
            .with_introspection_auth_method(IntrospectionAuthMethod::ClientSecretPost);
        r.introspect("abc").await.unwrap();
    }

    #[tokio::test]
    async fn introspect_basic_does_not_carry_credentials_in_form_body() {
        // Conjugate of the above — `ClientSecretBasic` (the default)
        // must NOT include `client_secret=` in the body.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .and(header("Authorization", "Basic Y2xpZW50OnRvcHNlY3JldA=="))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .expect(1)
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example").with_introspection_uri(
            format!("{}/introspect", server.uri()),
            "client",
            Some("topsecret".into()),
        );
        // Default = ClientSecretBasic.
        r.introspect("abc").await.unwrap();
    }

    #[test]
    fn discovery_doc_parses_optional_introspection_endpoint() {
        let with_endpoint: DiscoveryDoc = serde_json::from_value(json!({
            "issuer": "https://issuer.example",
            "jwks_uri": "https://issuer.example/jwks",
            "introspection_endpoint": "https://issuer.example/introspect",
        }))
        .unwrap();
        assert_eq!(
            with_endpoint.introspection_endpoint.as_deref(),
            Some("https://issuer.example/introspect"),
        );

        let without: DiscoveryDoc = serde_json::from_value(json!({
            "issuer": "https://issuer.example",
            "jwks_uri": "https://issuer.example/jwks",
        }))
        .unwrap();
        assert!(without.introspection_endpoint.is_none());
    }

    // -----------------------------------------------------------------
    // Phase 17.A — discovery-driven auth-method selection.
    // -----------------------------------------------------------------

    /// Build a `test_resolver` with introspection already wired up so
    /// the new auto-select builder isn't a silent no-op.
    fn test_resolver_with_introspection(
        client_secret: Option<&str>,
        advertised: Option<Vec<&str>>,
    ) -> OidcAuthResolver {
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_introspection_uri = Some("https://issuer.example/introspect".into());
        r.discovered_introspection_auth_methods =
            advertised.map(|v| v.into_iter().map(String::from).collect());
        r.with_introspection("client-id", client_secret.map(String::from))
            .unwrap()
    }

    #[test]
    fn discovery_doc_parses_optional_auth_methods_supported() {
        let with_methods: DiscoveryDoc = serde_json::from_value(json!({
            "issuer": "https://issuer.example",
            "jwks_uri": "https://issuer.example/jwks",
            "introspection_endpoint_auth_methods_supported":
                ["client_secret_basic", "client_secret_post"],
        }))
        .unwrap();
        assert_eq!(
            with_methods.introspection_endpoint_auth_methods_supported,
            Some(vec![
                "client_secret_basic".to_string(),
                "client_secret_post".to_string(),
            ]),
        );

        let without: DiscoveryDoc = serde_json::from_value(json!({
            "issuer": "https://issuer.example",
            "jwks_uri": "https://issuer.example/jwks",
        }))
        .unwrap();
        assert!(
            without
                .introspection_endpoint_auth_methods_supported
                .is_none()
        );
    }

    #[test]
    fn auto_select_no_op_without_introspection_config() {
        // No `with_introspection*` called — auto-select is a silent
        // no-op and returns `Ok(self)`.
        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_auth_method_from_discovery()
            .unwrap();
        assert!(r.introspection.is_none());
    }

    #[test]
    fn auto_select_picks_basic_when_field_absent() {
        // RFC 8414: when the issuer doesn't advertise the field, the
        // default is `client_secret_basic`.
        let r = test_resolver_with_introspection(Some("secret"), None)
            .with_introspection_auth_method_from_discovery()
            .unwrap();
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::ClientSecretBasic,
        );
    }

    #[test]
    fn auto_select_picks_basic_when_listed() {
        let r = test_resolver_with_introspection(Some("secret"), Some(vec!["client_secret_basic"]))
            .with_introspection_auth_method_from_discovery()
            .unwrap();
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::ClientSecretBasic,
        );
    }

    #[test]
    fn auto_select_picks_post_when_only_post_listed() {
        let r = test_resolver_with_introspection(Some("secret"), Some(vec!["client_secret_post"]))
            .with_introspection_auth_method_from_discovery()
            .unwrap();
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::ClientSecretPost,
        );
    }

    #[test]
    fn auto_select_errors_when_nothing_supported_advertised() {
        // Issuer requires only methods deferred to Phase 17.B+ / 18+
        // (`tls_client_auth`, `private_key_jwt`, `client_secret_jwt`).
        // Fail-closed at builder time so the operator notices.
        let r = test_resolver_with_introspection(
            Some("secret"),
            Some(vec!["tls_client_auth", "private_key_jwt"]),
        );
        let err = r
            .with_introspection_auth_method_from_discovery()
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("no supported introspection auth method"),
            "got: {msg}"
        );
        assert!(msg.contains("tls_client_auth"), "got: {msg}");
    }

    #[test]
    fn auto_select_prefers_basic_over_post() {
        let r = test_resolver_with_introspection(
            Some("secret"),
            Some(vec!["client_secret_post", "client_secret_basic"]),
        )
        .with_introspection_auth_method_from_discovery()
        .unwrap();
        // Even though `client_secret_post` appears first in the list,
        // we prefer Basic per RFC 7662 §2.1's "MUST support Basic"
        // precedent.
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::ClientSecretBasic,
        );
    }

    // -----------------------------------------------------------------
    // Phase 17.B — `IntrospectionAuthMethod::ClientSecretJwt`.
    // -----------------------------------------------------------------

    #[test]
    fn auto_select_prefers_jwt_when_listed_and_secret_present() {
        let r = test_resolver_with_introspection(
            Some("secret"),
            Some(vec![
                "client_secret_basic",
                "client_secret_post",
                "client_secret_jwt",
            ]),
        )
        .with_introspection_auth_method_from_discovery()
        .unwrap();
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::ClientSecretJwt,
        );
    }

    #[test]
    fn auto_select_skips_jwt_when_no_secret() {
        // `client_secret_jwt` is HS256-over-secret in Phase 17.B —
        // when no secret is configured, fall back to Basic if listed.
        let r = test_resolver_with_introspection(
            None,
            Some(vec!["client_secret_jwt", "client_secret_basic"]),
        )
        .with_introspection_auth_method_from_discovery()
        .unwrap();
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::ClientSecretBasic,
        );
    }

    #[test]
    fn auto_select_errors_when_jwt_only_listed_and_no_secret() {
        // Issuer advertised only `client_secret_jwt` but the operator
        // configured no client_secret — Phase 17.B requires HS256
        // over the symmetric secret, so this is unsupported.
        // Fail-closed rather than silently picking Basic (which the
        // issuer refused to advertise).
        let r = test_resolver_with_introspection(None, Some(vec!["client_secret_jwt"]));
        let err = r
            .with_introspection_auth_method_from_discovery()
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("no supported introspection auth method"));
    }

    #[tokio::test]
    async fn introspect_jwt_errors_when_secret_missing() {
        // `with_introspection_uri(uri, "client", None)` configures
        // `client_secret = None`; switching to `ClientSecretJwt`
        // should make `introspect()` fail at request time.
        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_uri("https://issuer.example/introspect", "client", None)
            .with_introspection_auth_method(IntrospectionAuthMethod::ClientSecretJwt);
        let err = r.introspect("any-token").await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("client_secret_jwt requires client_secret"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn introspect_jwt_carries_client_assertion_form_fields() {
        // Phase 17.B — `ClientSecretJwt` MUST send `client_assertion`
        // and `client_assertion_type` form fields, NOT
        // `Authorization: Basic`, NOT `client_secret=...`.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/introspect"))
            // RFC 7521 §4.2 — fixed assertion-type URI. The string is
            // form-encoded so `:` becomes `%3A`.
            .and(wiremock::matchers::body_string_contains(
                "client_assertion_type=urn%3Aietf%3Aparams%3Aoauth%3Aclient-assertion-type%3Ajwt-bearer",
            ))
            .and(wiremock::matchers::body_string_contains("client_assertion="))
            .and(wiremock::matchers::body_string_contains("token=abc"))
            // No `client_secret=` field — credentials live in the
            // signed JWT, not in plaintext form fields.
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .expect(1)
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_uri(
                format!("{}/introspect", server.uri()),
                "client",
                Some("topsecret".into()),
            )
            .with_introspection_auth_method(IntrospectionAuthMethod::ClientSecretJwt);
        r.introspect("abc").await.unwrap();
    }

    #[tokio::test]
    async fn introspect_jwt_signed_with_client_secret_hs256() {
        // Capture the posted body, parse out the `client_assertion`
        // JWT, verify the signature against the configured
        // `client_secret` using `jsonwebtoken::decode`, assert claims
        // (`iss` / `sub` = `client_id`, `aud` = `introspect_uri`,
        // `exp` in the near future).
        let server = MockServer::start().await;
        let introspect_uri = format!("{}/introspect", server.uri());
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .expect(1)
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_uri(&introspect_uri, "the-client-id", Some("the-secret".into()))
            .with_introspection_auth_method(IntrospectionAuthMethod::ClientSecretJwt);
        r.introspect("abc").await.unwrap();

        // Pull the captured request and parse out `client_assertion`.
        let received = server.received_requests().await.expect("requests");
        assert_eq!(received.len(), 1);
        let body = std::str::from_utf8(&received[0].body).expect("utf8 body");
        let assertion = url::form_urlencoded::parse(body.as_bytes())
            .find(|(k, _)| k == "client_assertion")
            .map(|(_, v)| v.into_owned())
            .expect("client_assertion form field");

        // Verify signature with the same client_secret bytes.
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(std::slice::from_ref(&introspect_uri));
        validation.set_issuer(&["the-client-id"]);
        validation.required_spec_claims.clear();
        validation.required_spec_claims.insert("exp".into());
        validation.required_spec_claims.insert("iss".into());
        validation.required_spec_claims.insert("aud".into());
        let key = DecodingKey::from_secret(b"the-secret");
        let data = decode::<BTreeMap<String, serde_json::Value>>(&assertion, &key, &validation)
            .expect("assertion verifies under the same secret");

        assert_eq!(
            data.claims.get("iss").and_then(|v| v.as_str()),
            Some("the-client-id"),
        );
        assert_eq!(
            data.claims.get("sub").and_then(|v| v.as_str()),
            Some("the-client-id"),
        );
        assert_eq!(
            data.claims.get("aud").and_then(|v| v.as_str()),
            Some(introspect_uri.as_str()),
        );
        // `exp` should be ~30s in the future of `iat`.
        let iat = data.claims.get("iat").and_then(|v| v.as_u64()).unwrap();
        let exp = data.claims.get("exp").and_then(|v| v.as_u64()).unwrap();
        assert_eq!(exp - iat, 30);
        assert!(data.claims.get("jti").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn make_jti_yields_unique_values() {
        // The monotonic counter component must disambiguate calls
        // within the same nanosecond.
        let n = 256;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..n {
            assert!(seen.insert(make_jti()), "duplicate jti");
        }
    }

    // -----------------------------------------------------------------
    // Phase 18.A — `IntrospectionAuthMethod::PrivateKeyJwt` and the
    // [`ClientAssertionKey`] surface.
    // -----------------------------------------------------------------

    /// PKCS#8 RSA-2048 private key generated for the Phase 18.A tests.
    /// Pairs with [`TEST_RSA_PUB_PEM`]. Test fixture only; never used
    /// for production signing.
    const TEST_RSA_PRIV_PEM: &[u8] = b"-----BEGIN PRIVATE KEY-----\n\
MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQC9+b2rgzJ5HJG+\n\
XxSlxoR/1JSZz1Q+FBg87F1AIR2CTtn0k2phG7f9ScAMvSPD8Vi5d7CNE2Dwy8sD\n\
GI9OUr6+HX3WDeRjPMmTtdhKUftSGtzT42zfmF3P8KCoIHH1bgCKCzQkbjbN4eTl\n\
LUsJplOqM6tLeveTbsBSFE3EQPujRjLshMGPhyWVJ4jWsDm62SQWGfHNkaao0pGJ\n\
vllScJe7EqlEwDw+TyXUb6eS0Zt6W9i2ACp0GuRSbh05gmjq66HAesTKyjqrAkJ6\n\
ZTmqo6s7kPsr8B/L5EoRN4UrYJ8UsSxCoj/CXxENSVpqYvdZsuRovQFAgfn3jy0f\n\
T74KkXS3AgMBAAECggEAAvt6MkiXe2uVkCRZpHaW2ujhQlQ3vC0VlP3tmCr3lcrw\n\
xKmnHbTRMR0+HL/ADCdBn3u/s59DwilOMRNqyy3Psm1abd3+9rMxbkEIZGD1becK\n\
Y3B/uOI2oCPkvxmZ9bfkiJuUwHj9zEKeah30E8fe1V5aneSQIWo3g7JaPC6m+fus\n\
+eyLD9yLLpz1qkyztykv2XpzO88bHjTFnJz0rRAsh/d1KEIY2lNMHO8zRX/tqQhU\n\
8/8SIvg4WEDdpkQ56+ouMoOSundZ6QG1MZlrisLDUs4A9zeTJuFFe6cW8bEFrvH3\n\
/CekZkRD0NmX0h6GbKKU0Axqlt3WgeJKFe8Mr8rCDQKBgQD+bh2ZuGe985Z+lrm1\n\
g4d2gP4gu8V57KsSQlQ/CIMTmHlCPWrbekVX/6ONV/MRc3WZHMfsx+qxOY9IQp+u\n\
VyGnQDpVsTcIRuOMWqdXbvwtverxp/gXZOiFGbadF4HDx+8q6Q4cI/3xCtXx5bfg\n\
UOXLSApYFJXP9BVX3H+IFppKpQKBgQC/JdDzkbctcyq0F3qY/FSJdi4C2mQPNoVZ\n\
hw420OlYeQcFT5hzR/Ye5AWng1oqZ5yO5GaH5T8XpmZXcMIV/E/Rmo9VleKPvGx2\n\
BDfbI1FgJ26bMHpTWfsOmLly5M82NlNg06Mt97VU+xd3Wn1A9o10ColyJvbP4i8R\n\
i6+AbsZPKwKBgQCoBfBmY/Ge8A6i6scZqBL9n5Iz680uB62yETuxpN1rQ3ZQ2F6J\n\
MuY4hwprfXl4PNeclfUx2ZSUFX8aKWVqrP/8g94CWVYOkUIUnomEpDbFvnY5wMOG\n\
L42e2KxQcgWwVYkMvXwj+WDqnk1Lwnj8GnCnHpw2LuIAwyCVNXjDVqnuQQKBgQCH\n\
7s6vyDpqGfKOa/wFe7xqnR6PbNunbfBbAI59MQggoMD7Z+VUZiKDSUk0HVcrvM87\n\
VvYLQl4h5XX2TPvZQrtIpg+0n4ilCyxeqRVHw9AE/0XLGyiCygSeFsIbENjDBtM4\n\
kokDEZtkucOwXyuf3TYvBadFBKyUnZc3dQzz2tMwTQKBgQDoeHgjcKYcajW996Zq\n\
Ytc71DwaT6+cpJbiEBVLH8O40Adwe8ne0oZDPYzFhOtOhDryQtTzbSub4/WnfPUH\n\
RfN037ltl89z5VGHUUnFiaxhCGY2DDFeVhz1jODsoUrsNFqKLJbwY6IQJMMHUnP7\n\
WNwjxNHiEmjTyiooqJmBvBm5Eg==\n\
-----END PRIVATE KEY-----\n";

    /// Public key matching [`TEST_RSA_PRIV_PEM`].
    const TEST_RSA_PUB_PEM: &[u8] = b"-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAvfm9q4MyeRyRvl8UpcaE\n\
f9SUmc9UPhQYPOxdQCEdgk7Z9JNqYRu3/UnADL0jw/FYuXewjRNg8MvLAxiPTlK+\n\
vh191g3kYzzJk7XYSlH7Uhrc0+Ns35hdz/CgqCBx9W4Aigs0JG42zeHk5S1LCaZT\n\
qjOrS3r3k27AUhRNxED7o0Yy7ITBj4cllSeI1rA5utkkFhnxzZGmqNKRib5ZUnCX\n\
uxKpRMA8Pk8l1G+nktGbelvYtgAqdBrkUm4dOYJo6uuhwHrEyso6qwJCemU5qqOr\n\
O5D7K/Afy+RKETeFK2CfFLEsQqI/wl8RDUlaamL3WbLkaL0BQIH5948tH0++CpF0\n\
twIDAQAB\n\
-----END PUBLIC KEY-----\n";

    /// PKCS#8 EC P-256 private key for the Phase 18.A ES256 smoke
    /// test. We don't verify a JWT signed by it (that would need a
    /// matched public key + an extra wiremock test); we just confirm
    /// the constructor accepts the PEM.
    const TEST_EC_PRIV_PEM: &[u8] = b"-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgcXj4rM9/34aqTR2u\n\
OKhNOiUerTQ/GPZ6q8rcrfmiykuhRANCAAToAq4Npj+odbH4wU2daxYN3tcoJool\n\
Hg1uvux+qhVvB6JSr1th1Vbqvs7mLJioou3cLxSuM/AqPKOyBmWl2hf5\n\
-----END PRIVATE KEY-----\n";

    #[test]
    fn client_assertion_key_from_rs256_pem_round_trip() {
        let k = ClientAssertionKey::from_rs256_pem(TEST_RSA_PRIV_PEM).unwrap();
        assert_eq!(k.algorithm(), Algorithm::RS256);
        // Debug must redact the key body but expose the algorithm.
        let dbg = format!("{k:?}");
        assert!(dbg.contains("RS256"), "got: {dbg}");
        assert!(dbg.contains("redacted"), "got: {dbg}");
    }

    #[test]
    fn client_assertion_key_from_es256_pem_round_trip() {
        let k = ClientAssertionKey::from_es256_pem(TEST_EC_PRIV_PEM).unwrap();
        assert_eq!(k.algorithm(), Algorithm::ES256);
    }

    #[test]
    fn client_assertion_key_rejects_garbage_pem() {
        let err = ClientAssertionKey::from_rs256_pem(b"not a pem").unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("invalid RS256"), "got: {msg}");
    }

    #[test]
    fn auto_select_prefers_private_key_jwt_when_listed_and_key_present() {
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_introspection_uri = Some("https://issuer.example/introspect".into());
        r.discovered_introspection_auth_methods = Some(vec![
            "client_secret_basic".to_string(),
            "client_secret_post".to_string(),
            "client_secret_jwt".to_string(),
            "private_key_jwt".to_string(),
        ]);
        let key = ClientAssertionKey::from_rs256_pem(TEST_RSA_PRIV_PEM).unwrap();
        let r = r
            .with_introspection("client", Some("secret".into()))
            .unwrap()
            .with_introspection_private_key(key)
            .with_introspection_auth_method_from_discovery()
            .unwrap();
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::PrivateKeyJwt,
        );
    }

    #[test]
    fn auto_select_skips_private_key_jwt_when_no_key() {
        // `private_key_jwt` is listed but no key configured — fall
        // back to `client_secret_jwt` (also listed; secret present).
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_introspection_uri = Some("https://issuer.example/introspect".into());
        r.discovered_introspection_auth_methods = Some(vec![
            "private_key_jwt".to_string(),
            "client_secret_jwt".to_string(),
        ]);
        let r = r
            .with_introspection("client", Some("secret".into()))
            .unwrap()
            .with_introspection_auth_method_from_discovery()
            .unwrap();
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::ClientSecretJwt,
        );
    }

    #[tokio::test]
    async fn introspect_private_key_jwt_errors_when_key_missing() {
        // Auth method is `PrivateKeyJwt` but no `client_assertion_key`
        // was configured — `introspect()` errors at request time.
        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_uri("https://issuer.example/introspect", "client", None)
            .with_introspection_auth_method(IntrospectionAuthMethod::PrivateKeyJwt);
        let err = r.introspect("any-token").await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("private_key_jwt requires client_assertion_key"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn introspect_private_key_jwt_carries_client_assertion_form_fields() {
        // Phase 18.A — `PrivateKeyJwt` MUST send `client_assertion`
        // and `client_assertion_type` form fields, NOT
        // `Authorization: Basic`, NOT `client_secret=`.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .and(wiremock::matchers::body_string_contains(
                "client_assertion_type=urn%3Aietf%3Aparams%3Aoauth%3Aclient-assertion-type%3Ajwt-bearer",
            ))
            .and(wiremock::matchers::body_string_contains("client_assertion="))
            .and(wiremock::matchers::body_string_contains("token=abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .expect(1)
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_uri(format!("{}/introspect", server.uri()), "client", None)
            .with_introspection_jwt_rs256_pem(TEST_RSA_PRIV_PEM)
            .unwrap();
        r.introspect("abc").await.unwrap();
    }

    #[tokio::test]
    async fn introspect_private_key_jwt_signed_with_rs256() {
        // Capture the posted body, parse out the `client_assertion`
        // JWT, verify the RS256 signature against the matching public
        // key, and assert claims (`iss` / `sub` = `client_id`,
        // `aud` = `introspect_uri`, `exp` ~30s in the future).
        let server = MockServer::start().await;
        let introspect_uri = format!("{}/introspect", server.uri());
        Mock::given(method("POST"))
            .and(path("/introspect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .expect(1)
            .mount(&server)
            .await;

        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_uri(&introspect_uri, "the-client-id", None)
            .with_introspection_jwt_rs256_pem(TEST_RSA_PRIV_PEM)
            .unwrap();
        r.introspect("abc").await.unwrap();

        // Pull the captured request and parse out `client_assertion`.
        let received = server.received_requests().await.expect("requests");
        assert_eq!(received.len(), 1);
        let body = std::str::from_utf8(&received[0].body).expect("utf8 body");
        let assertion = url::form_urlencoded::parse(body.as_bytes())
            .find(|(k, _)| k == "client_assertion")
            .map(|(_, v)| v.into_owned())
            .expect("client_assertion form field");

        // Verify signature against the matching public key.
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(std::slice::from_ref(&introspect_uri));
        validation.set_issuer(&["the-client-id"]);
        validation.required_spec_claims.clear();
        validation.required_spec_claims.insert("exp".into());
        validation.required_spec_claims.insert("iss".into());
        validation.required_spec_claims.insert("aud".into());
        let key = DecodingKey::from_rsa_pem(TEST_RSA_PUB_PEM).expect("valid pub PEM");
        let data = decode::<BTreeMap<String, serde_json::Value>>(&assertion, &key, &validation)
            .expect("assertion verifies under matching public key");

        assert_eq!(
            data.claims.get("iss").and_then(|v| v.as_str()),
            Some("the-client-id"),
        );
        assert_eq!(
            data.claims.get("sub").and_then(|v| v.as_str()),
            Some("the-client-id"),
        );
        assert_eq!(
            data.claims.get("aud").and_then(|v| v.as_str()),
            Some(introspect_uri.as_str()),
        );
        let iat = data.claims.get("iat").and_then(|v| v.as_u64()).unwrap();
        let exp = data.claims.get("exp").and_then(|v| v.as_u64()).unwrap();
        assert_eq!(exp - iat, 30);
        assert!(data.claims.get("jti").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn auto_select_errors_when_only_unsupported_methods_advertised_phase18() {
        // After Phase 18.A `private_key_jwt` is supported when a key
        // is present. The fail-closed path now fires only on
        // `tls_client_auth` / unknown methods.
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_introspection_uri = Some("https://issuer.example/introspect".into());
        r.discovered_introspection_auth_methods = Some(vec!["tls_client_auth".to_string()]);
        let r = r
            .with_introspection("client", Some("secret".into()))
            .unwrap();
        let err = r
            .with_introspection_auth_method_from_discovery()
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("no supported introspection auth method"));
    }

    // -----------------------------------------------------------------
    // Phase 18.B — OIDC Session Management 1.0 end-session helper.
    // -----------------------------------------------------------------

    #[test]
    fn discovery_doc_parses_optional_end_session_endpoint() {
        let with_endpoint: DiscoveryDoc = serde_json::from_value(json!({
            "issuer": "https://issuer.example",
            "jwks_uri": "https://issuer.example/jwks",
            "end_session_endpoint": "https://issuer.example/logout",
        }))
        .unwrap();
        assert_eq!(
            with_endpoint.end_session_endpoint.as_deref(),
            Some("https://issuer.example/logout"),
        );

        let without: DiscoveryDoc = serde_json::from_value(json!({
            "issuer": "https://issuer.example",
            "jwks_uri": "https://issuer.example/jwks",
        }))
        .unwrap();
        assert!(without.end_session_endpoint.is_none());
    }

    #[test]
    fn end_session_endpoint_accessor_returns_captured_uri() {
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        assert!(r.end_session_endpoint().is_none());
        r.discovered_end_session_uri = Some("https://issuer.example/logout".into());
        assert_eq!(
            r.end_session_endpoint(),
            Some("https://issuer.example/logout"),
        );
    }

    #[test]
    fn build_logout_uri_returns_none_when_not_advertised() {
        let r = test_resolver(Client::new(), "https://issuer.example");
        assert!(r.build_logout_uri(None, None, None).is_none());
        assert!(r.build_logout_uri(Some("hint"), None, None).is_none());
    }

    #[test]
    fn build_logout_uri_with_no_params_yields_bare_endpoint() {
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_end_session_uri = Some("https://issuer.example/logout".into());
        assert_eq!(
            r.build_logout_uri(None, None, None).as_deref(),
            Some("https://issuer.example/logout"),
        );
    }

    #[test]
    fn build_logout_uri_with_all_params() {
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_end_session_uri = Some("https://issuer.example/logout".into());
        let uri = r
            .build_logout_uri(
                Some("the-id-token"),
                Some("https://app.example/post-logout"),
                Some("xyz"),
            )
            .unwrap();
        // OIDC Session Management 1.0 §5 — all three params should
        // appear, URL-encoded as needed.
        assert!(uri.starts_with("https://issuer.example/logout?"));
        assert!(uri.contains("id_token_hint=the-id-token"), "got: {uri}");
        assert!(
            uri.contains("post_logout_redirect_uri=https%3A%2F%2Fapp.example%2Fpost-logout",),
            "got: {uri}",
        );
        assert!(uri.contains("state=xyz"), "got: {uri}");
    }

    #[test]
    fn build_logout_uri_appends_with_ampersand_when_query_present() {
        // RFC 3986: `?` only at the start of the query; subsequent
        // params join with `&`.
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_end_session_uri = Some("https://issuer.example/logout?tenant=acme".into());
        let uri = r.build_logout_uri(Some("hint"), None, None).unwrap();
        assert_eq!(
            uri,
            "https://issuer.example/logout?tenant=acme&id_token_hint=hint",
        );
    }

    #[test]
    fn build_logout_uri_omits_none_params() {
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_end_session_uri = Some("https://issuer.example/logout".into());
        let uri = r.build_logout_uri(Some("hint"), None, Some("xyz")).unwrap();
        assert!(uri.contains("id_token_hint=hint"), "got: {uri}");
        assert!(uri.contains("state=xyz"), "got: {uri}");
        assert!(!uri.contains("post_logout_redirect_uri"), "got: {uri}");
    }

    // -----------------------------------------------------------------
    // Phase 24 — `IntrospectionAuthMethod::TlsClientAuth` and
    // [`OidcAuthResolver::with_introspection_mtls`].
    // -----------------------------------------------------------------

    /// Self-signed X.509 cert generated for the Phase 24 tests.
    /// CN = `tako-test-client`, RSA-2048, validity ~100 years.
    /// Test fixture only; never used for production auth.
    const TEST_MTLS_CERT_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\n\
MIIDGTCCAgGgAwIBAgIUDgBxyYdSvB715hZ2wo2vg58ajPEwDQYJKoZIhvcNAQEL\n\
BQAwGzEZMBcGA1UEAwwQdGFrby10ZXN0LWNsaWVudDAgFw0yNjA1MDEwODQyNDJa\n\
GA8yMTI2MDQwNzA4NDI0MlowGzEZMBcGA1UEAwwQdGFrby10ZXN0LWNsaWVudDCC\n\
ASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAKn2gHS5FrOc6Kjx1aZDzmpB\n\
3CLeVWMYfXtVJO+p6mtJkIYUhqPLt1BesbdABmIBByjghtlenEP9xbOYbEe5qPxQ\n\
ihy9VmgITrq3DXUhdZhCxGHp99dzLPaE1XBaUHH3eYqlbbxd8dc1qRiULA6/f7mR\n\
92q6sZzUp5znDRwvwRGgf0x3JowfzeIetoKtNJ/RH1LmyCeqGd1djtyVe/2atsbL\n\
6DfDoEdT4en0WcIkZGtw9LYKvTImCidqTd8N+dpNSMPJTn4KctVHXmOdBpDK5U/u\n\
XF+SsGFg+4lFO/JTGTCowBGv7KeIoBf5vrJe9w/L01rCExnZhYQVTs8wjNZ1VBsC\n\
AwEAAaNTMFEwHQYDVR0OBBYEFEqlQohkpj9ddcSoQ57Onk0c5iwGMB8GA1UdIwQY\n\
MBaAFEqlQohkpj9ddcSoQ57Onk0c5iwGMA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZI\n\
hvcNAQELBQADggEBACtlr1SIz6YsRijnj9oMhGei1CVRXRnFHD8z2poa7A1Zh3vC\n\
nFdBOACHpmJ++A8Z1xOFyM064U/lYNybFw0/kyhk+9x5LlV3XCnT2r3CjVeacyfF\n\
kWy8kmaZ2j6JRTL/O0j8+ZlSZkf4utt/3+uGFUQ/qmmnXsYbhsyvHpnUmhZAnQxc\n\
Y+zVlpb9xALf3F2RuHZmhngdbIBaRFuExhcnktIdHbUUCq+Lc45or0gCk1yqf2GX\n\
+PIVp3MWA9hwQP3Obx88GzGaLZ/MpfzE41vVjtlnyBirt0lFqAyM8JT+vFjcrg0n\n\
ZVBd2WsafuufFwi8IZInM7P/gTi57eNbhpQMYzc=\n\
-----END CERTIFICATE-----\n";

    /// PKCS#8 RSA private key matching [`TEST_MTLS_CERT_PEM`].
    const TEST_MTLS_KEY_PEM: &[u8] = b"-----BEGIN PRIVATE KEY-----\n\
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCp9oB0uRaznOio\n\
8dWmQ85qQdwi3lVjGH17VSTvqeprSZCGFIajy7dQXrG3QAZiAQco4IbZXpxD/cWz\n\
mGxHuaj8UIocvVZoCE66tw11IXWYQsRh6ffXcyz2hNVwWlBx93mKpW28XfHXNakY\n\
lCwOv3+5kfdqurGc1Kec5w0cL8ERoH9MdyaMH83iHraCrTSf0R9S5sgnqhndXY7c\n\
lXv9mrbGy+g3w6BHU+Hp9FnCJGRrcPS2Cr0yJgonak3fDfnaTUjDyU5+CnLVR15j\n\
nQaQyuVP7lxfkrBhYPuJRTvyUxkwqMARr+yniKAX+b6yXvcPy9NawhMZ2YWEFU7P\n\
MIzWdVQbAgMBAAECggEAUnySalO7207/MaMw5AELiFFPY9LQ0Qe9OqKfivtFjG1H\n\
CXOjxpHjdUuH555Ymq7SCToy6AL9Rxg+H4QNpR/Lji0OYpVXfqTthLu7ecnT1yIs\n\
SjLxeGxq+XeNWPpUCYOoRqwz3lQfv6lI2GdtHHk/JVJcqD1UXv9sG3+dQr1Ab+tQ\n\
tVmVRNHA7E297v5kwYjKxEvobBjtRqDS3mVh21Fcfd+YNvAzbQ5MJpc1fqJ6TzLD\n\
4vs3yNZ2Utww4ItMFi1jf4AGxJ+s9887rJffV96g8fmaAAVJPHX6aHj+J2yibLiY\n\
TBpImZgd5x/sis9nNQdfA4749gb/vn/d+wt5Nq8o4QKBgQDbHtW/eDIJzQrj4djB\n\
pJXvGiQzp7dwgd5zxjpRmMpWMOymyJfu4LOW0hGH+YOmKV4DTdJ0OsNlpnccsQFT\n\
d0Xnpbmz0KXDybaUwqEsExpkNiPruC3Nq5ID03l89q1usyoLZfYPSWESfazmjG6h\n\
VlS2kKLwrTK3pLdKWbEBLIptewKBgQDGkaICZdgLbu5zbTszUi0zjZuVH59lIu3+\n\
CrxgGdjPTyCrot0qzxiYWnc6auvm8VVKoO8YqWyGaknwUmI8AwU9tETgTh4cv6gu\n\
YzOr6EhBYfNkUoTAkdyDu7Vbje7zSJY8YsjJCrdazj5gIOq4hLazXE9JFQnBoWln\n\
BQjXehbh4QKBgQDDHiQMCYXVQGZwIc4YMOzqKwcNkE1CvAJQabXIrxuNwKcapQjV\n\
x/VjWdAOmtrl/XQf0Q6UPTd9rsvmGqApqM3wxpwkSKkzPM1+jgli6+fWUHeQEUOI\n\
Hz04dvl5k1dAef34hGSlnBv6kTqDWY2x0ORCZW0Sj8fXy68DX/bEKtthPQKBgExe\n\
NDXB334+Mrz31J3fS/0YyC5pFA98iJV8oYhASI8qeoEoSPEu5uGpYVN5TbLrPAdQ\n\
r8QHXPKxLDCeLqOv8bMSgq7VvGUIHPGCO5ww4KEsv8PkrKO3NV0AszY79xtf3k/p\n\
Ghmf4nas/XZREpTWjcGbje6ohbEPmA8D86uTi/thAoGAZVuIdoETvKNVpT0O0qBX\n\
yxjBYrLoXdkns6ZR5I+jD42jvtv9UkASFydzHI6k5ZCJ38HN7hRoLHCSB46cEcOX\n\
GyKEFEUINrmViWeq1ysFaNzOu0EjypVCwvN6/Jx8kmfNFuHGdpqjoaNRecAYyGOr\n\
h7ACptP3tF94pcBzOgJ3bhM=\n\
-----END PRIVATE KEY-----\n";

    fn test_resolver_with_introspection_for_mtls() -> OidcAuthResolver {
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_introspection_uri = Some("https://issuer.example/introspect".into());
        r.with_introspection("client-id", None).unwrap()
    }

    #[test]
    fn with_introspection_mtls_accepts_valid_pem() {
        let r = test_resolver_with_introspection_for_mtls()
            .with_introspection_mtls(TEST_MTLS_CERT_PEM, TEST_MTLS_KEY_PEM)
            .unwrap();
        let cfg = r.introspection.expect("introspection set");
        assert_eq!(cfg.auth_method, IntrospectionAuthMethod::TlsClientAuth);
        assert!(cfg.mtls_client.is_some());
    }

    #[test]
    fn with_introspection_mtls_combined_accepts_concatenated_pem() {
        // Concat cert + key into a single blob — the common output of
        // `cat cert.pem key.pem`.
        let mut combined = Vec::with_capacity(TEST_MTLS_CERT_PEM.len() + TEST_MTLS_KEY_PEM.len());
        combined.extend_from_slice(TEST_MTLS_CERT_PEM);
        combined.extend_from_slice(TEST_MTLS_KEY_PEM);

        let r = test_resolver_with_introspection_for_mtls()
            .with_introspection_mtls_combined(&combined)
            .unwrap();
        let cfg = r.introspection.expect("introspection set");
        assert_eq!(cfg.auth_method, IntrospectionAuthMethod::TlsClientAuth);
        assert!(cfg.mtls_client.is_some());
    }

    #[test]
    fn with_introspection_mtls_rejects_garbage_pem() {
        let err = test_resolver_with_introspection_for_mtls()
            .with_introspection_mtls(b"not a pem", b"also not a pem")
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("invalid mTLS identity PEM"), "got: {msg}");
    }

    #[test]
    fn with_introspection_mtls_no_op_without_introspection() {
        // No `with_introspection*` called yet — the mTLS builder
        // returns `Ok(self)` without trying to parse the PEM (so
        // operators can chain in any order without surfacing
        // unrelated PEM errors).
        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_mtls(b"garbage", b"garbage")
            .unwrap();
        assert!(r.introspection.is_none());
    }

    #[test]
    fn auto_select_prefers_tls_client_auth_when_listed_and_identity_present() {
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_introspection_uri = Some("https://issuer.example/introspect".into());
        r.discovered_introspection_auth_methods = Some(vec![
            "client_secret_basic".to_string(),
            "client_secret_post".to_string(),
            "client_secret_jwt".to_string(),
            "private_key_jwt".to_string(),
            "tls_client_auth".to_string(),
        ]);
        let r = r
            .with_introspection("client", Some("secret".into()))
            .unwrap()
            .with_introspection_mtls(TEST_MTLS_CERT_PEM, TEST_MTLS_KEY_PEM)
            .unwrap()
            .with_introspection_auth_method_from_discovery()
            .unwrap();
        // Even though all five methods are listed and we have
        // secret + (no asymmetric key), mTLS is the strongest
        // method tako can perform here.
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::TlsClientAuth,
        );
    }

    #[test]
    fn auto_select_skips_tls_client_auth_when_no_identity() {
        // `tls_client_auth` is listed but no mTLS identity
        // configured — fall back to `client_secret_basic`.
        let mut r = test_resolver(Client::new(), "https://issuer.example");
        r.discovered_introspection_uri = Some("https://issuer.example/introspect".into());
        r.discovered_introspection_auth_methods = Some(vec![
            "tls_client_auth".to_string(),
            "client_secret_basic".to_string(),
        ]);
        let r = r
            .with_introspection("client", Some("secret".into()))
            .unwrap()
            .with_introspection_auth_method_from_discovery()
            .unwrap();
        assert_eq!(
            r.introspection.unwrap().auth_method,
            IntrospectionAuthMethod::ClientSecretBasic,
        );
    }

    #[tokio::test]
    async fn introspect_tls_client_auth_errors_when_mtls_client_missing() {
        // Auth method flipped to `TlsClientAuth` directly (without
        // `with_introspection_mtls`) — `introspect()` errors at
        // request time.
        let r = test_resolver(Client::new(), "https://issuer.example")
            .with_introspection_uri("https://issuer.example/introspect", "client", None)
            .with_introspection_auth_method(IntrospectionAuthMethod::TlsClientAuth);
        let err = r.introspect("any-token").await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("tls_client_auth requires mtls_client"),
            "got: {msg}"
        );
    }
}
