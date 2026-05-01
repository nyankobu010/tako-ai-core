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
//! Out-of-scope (deferred to Phase 17+): refresh-token flows,
//! end-session endpoint, discovery-driven selection (reading
//! `introspection_endpoint_auth_methods_supported` per RFC 8414),
//! `client_secret_jwt` and mTLS (`tls_client_auth`) auth methods.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
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
}

/// Phase 16.B.2 — RFC 7662 §2.1 introspection endpoint auth method.
///
/// Selected via [`OidcAuthResolver::with_introspection_auth_method`].
/// `ClientSecretBasic` (the default) carries credentials in the
/// `Authorization: Basic ...` header; `ClientSecretPost` carries
/// them as additional fields in the form-encoded request body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IntrospectionAuthMethod {
    /// HTTP Basic auth — Phase 15.B.2 default behaviour.
    #[default]
    ClientSecretBasic,
    /// Credentials sent as `client_id` / `client_secret` form fields
    /// alongside `token`. Per RFC 7662 §2.1 the server MUST accept
    /// either method when authenticating a confidential client.
    ClientSecretPost,
}

/// Phase 15.B.2 — RFC 7662 token-introspection configuration.
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

    /// Phase 15.B.2 — POST the token to the introspection endpoint
    /// and confirm `active=true`. Returns `Err` with a
    /// `TakoError::Invalid("oidc: token revoked ...")` payload when
    /// the issuer rejects the token.
    async fn introspect(&self, token: &str) -> Result<(), TakoError> {
        let Some(cfg) = &self.introspection else {
            return Ok(());
        };
        // Workspace reqwest is configured without the `urlencoded`
        // feature; build the form body manually via `url`'s
        // `form_urlencoded`. This is what reqwest's `.form()` does
        // internally.
        //
        // Phase 16.B.2 — `ClientSecretPost` adds `client_id` and
        // `client_secret` form fields here; `ClientSecretBasic` keeps
        // the body credential-free and adds `Authorization: Basic`
        // below. The `Serializer` is not `Send`, so build the body
        // string in a tight scope that drops it before any await.
        let body = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            form.append_pair("token", token)
                .append_pair("token_type_hint", "access_token");
            if cfg.auth_method == IntrospectionAuthMethod::ClientSecretPost {
                form.append_pair("client_id", &cfg.client_id);
                if let Some(secret) = &cfg.client_secret {
                    form.append_pair("client_secret", secret);
                }
            }
            form.finish()
        };
        let req = self
            .http
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
            IntrospectionAuthMethod::ClientSecretPost => req,
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
}
