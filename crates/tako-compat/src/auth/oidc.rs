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
//! Out-of-scope (deferred to Phase 15+): RFC 7662 token introspection,
//! refresh-token flows, end-session endpoint.

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
}

/// JWKS fetched from `jwks_uri`. Validation hint: a missing-`kid`
/// failure triggers a single force-refresh and retry.
#[derive(Debug)]
struct CachedJwks {
    jwks: JwkSet,
    fetched_at: Instant,
}

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
        match self.validate_against(token, &jwks).await {
            Ok(p) => Ok(p),
            Err(TakoError::Invalid(msg))
                if msg.contains("InvalidSignature")
                    || msg.contains("no JWK in cache matches")
                    || msg.contains("InvalidKid") =>
            {
                let fresh = self.jwks(true).await?;
                self.validate_against(token, &fresh).await
            }
            Err(other) => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    #[test]
    fn oidc_resolver_is_send_sync() {
        assert_send_sync::<OidcAuthResolver>();
    }
}
