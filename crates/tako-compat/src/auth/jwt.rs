//! `JwtAuthResolver` — verifies a signed JWT against a configured key.
//!
//! Phase 14.B. Supports HS256 (shared secret), RS256 (RSA pubkey PEM),
//! and ES256 (EC pubkey PEM). The signature algorithm is fixed at
//! resolver-construction time so an attacker cannot downgrade by
//! substituting an `alg` header.
//!
//! Claims layout (defaults; override via [`Self::with_claims`]):
//! - `tenant_id` — required string; populates `Principal::tenant_id`.
//! - `sub`       — required string; populates `Principal::user_id`.
//! - `roles`     — optional `Vec<String>`; populates `Principal::roles`.
//!
//! Errors map to `TakoError::Invalid("jwt: ...")` so the existing
//! [`crate::routes::resolve_principal`] 401-mapping works unchanged.
//!
//! # Example
//!
//! ```no_run
//! use tako_compat::JwtAuthResolver;
//!
//! let auth = JwtAuthResolver::hs256(b"super-secret")
//!     .with_audience("tako-api")
//!     .with_issuer("https://idp.example.com");
//! ```

use std::collections::BTreeMap;

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;
use tako_core::{Principal, TakoError};

use super::AuthResolver;

/// Default claim names — override with [`JwtAuthResolver::with_claims`].
const DEFAULT_TENANT_CLAIM: &str = "tenant_id";
const DEFAULT_USER_CLAIM: &str = "sub";
const DEFAULT_ROLES_CLAIM: &str = "roles";

/// Verifies a signed JWT and maps its claims to a [`Principal`].
pub struct JwtAuthResolver {
    decoding_key: DecodingKey,
    validation: Validation,
    tenant_claim: String,
    user_claim: String,
    roles_claim: String,
}

impl std::fmt::Debug for JwtAuthResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtAuthResolver")
            .field("alg", &self.validation.algorithms)
            .field("tenant_claim", &self.tenant_claim)
            .field("user_claim", &self.user_claim)
            .field("roles_claim", &self.roles_claim)
            .finish_non_exhaustive()
    }
}

impl JwtAuthResolver {
    /// HS256 with a shared secret.
    pub fn hs256(secret: &[u8]) -> Self {
        Self::with_alg(Algorithm::HS256, DecodingKey::from_secret(secret))
    }

    /// RS256 against an RSA public-key PEM.
    pub fn rs256_from_pem(pem: &[u8]) -> Result<Self, TakoError> {
        let key = DecodingKey::from_rsa_pem(pem)
            .map_err(|e| TakoError::Invalid(format!("jwt: invalid RS256 key: {e}")))?;
        Ok(Self::with_alg(Algorithm::RS256, key))
    }

    /// ES256 against an EC public-key PEM.
    pub fn es256_from_pem(pem: &[u8]) -> Result<Self, TakoError> {
        let key = DecodingKey::from_ec_pem(pem)
            .map_err(|e| TakoError::Invalid(format!("jwt: invalid ES256 key: {e}")))?;
        Ok(Self::with_alg(Algorithm::ES256, key))
    }

    fn with_alg(alg: Algorithm, key: DecodingKey) -> Self {
        let mut validation = Validation::new(alg);
        // The shipped default validates `exp` (which `Validation::new`
        // sets up) and pins the algorithm to the configured one above
        // — a JWT signed with a different algorithm is rejected before
        // the signature check, blocking `alg=none` and HS/RS confusion.
        validation.required_spec_claims.clear();
        validation.required_spec_claims.insert("exp".into());
        Self {
            decoding_key: key,
            validation,
            tenant_claim: DEFAULT_TENANT_CLAIM.into(),
            user_claim: DEFAULT_USER_CLAIM.into(),
            roles_claim: DEFAULT_ROLES_CLAIM.into(),
        }
    }

    /// Require the `aud` claim to match. Multiple calls overwrite the
    /// previous audience — the resolver enforces a single audience.
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self {
        let aud_str: String = aud.into();
        self.validation.set_audience(&[aud_str]);
        self
    }

    /// Require the `iss` claim to match.
    pub fn with_issuer(mut self, iss: impl Into<String>) -> Self {
        let iss_str: String = iss.into();
        self.validation.set_issuer(&[iss_str]);
        self
    }

    /// Override the claim names that map to `Principal` fields.
    pub fn with_claims(mut self, tenant: &str, user: &str, roles: &str) -> Self {
        self.tenant_claim = tenant.into();
        self.user_claim = user.into();
        self.roles_claim = roles.into();
        self
    }
}

/// Internal claim envelope — the resolver pulls fields by name out of
/// the deserialised map so callers can rename `tenant_id` / `sub` /
/// `roles` without us having to declare a new struct.
#[derive(Debug, Deserialize)]
struct ClaimEnvelope(BTreeMap<String, serde_json::Value>);

#[async_trait]
impl AuthResolver for JwtAuthResolver {
    async fn resolve(&self, token: &str) -> Result<Principal, TakoError> {
        let data = decode::<ClaimEnvelope>(token, &self.decoding_key, &self.validation)
            .map_err(|e| TakoError::Invalid(format!("jwt: {e}")))?;
        let claims = data.claims.0;
        let tenant_id = claims
            .get(&self.tenant_claim)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                TakoError::Invalid(format!(
                    "jwt: missing or non-string claim `{}`",
                    self.tenant_claim
                ))
            })?
            .to_string();
        let user_id = claims
            .get(&self.user_claim)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                TakoError::Invalid(format!(
                    "jwt: missing or non-string claim `{}`",
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
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_plus_secs(secs: u64) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + secs
    }

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    #[test]
    fn jwt_resolver_is_send_sync() {
        assert_send_sync::<JwtAuthResolver>();
    }

    #[tokio::test]
    async fn hs256_round_trip_yields_principal() {
        let secret = b"this-is-a-test-secret-that-is-long-enough";
        let auth = JwtAuthResolver::hs256(secret);

        let claims = json!({
            "tenant_id": "acme",
            "sub": "alice",
            "roles": ["admin", "ops"],
            "exp": now_plus_secs(60),
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let p = auth.resolve(&token).await.unwrap();
        assert_eq!(p.tenant_id, "acme");
        assert_eq!(p.user_id, "alice");
        assert_eq!(p.roles, vec!["admin".to_string(), "ops".to_string()]);
    }

    #[tokio::test]
    async fn hs256_invalid_signature_is_rejected() {
        let secret_a = b"secret-aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let secret_b = b"secret-bbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let auth = JwtAuthResolver::hs256(secret_b);

        let claims = json!({
            "tenant_id": "acme",
            "sub": "alice",
            "exp": now_plus_secs(60),
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret_a),
        )
        .unwrap();

        let err = auth.resolve(&token).await.unwrap_err();
        assert!(matches!(err, TakoError::Invalid(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn hs256_audience_mismatch_is_rejected() {
        let secret = b"this-is-a-test-secret-that-is-long-enough";
        let auth = JwtAuthResolver::hs256(secret).with_audience("tako-api");

        let claims = json!({
            "tenant_id": "acme",
            "sub": "alice",
            "exp": now_plus_secs(60),
            "aud": "other-api",
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let err = auth.resolve(&token).await.unwrap_err();
        assert!(matches!(err, TakoError::Invalid(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn hs256_audience_match_passes() {
        let secret = b"this-is-a-test-secret-that-is-long-enough";
        let auth = JwtAuthResolver::hs256(secret).with_audience("tako-api");

        let claims = json!({
            "tenant_id": "acme",
            "sub": "alice",
            "exp": now_plus_secs(60),
            "aud": "tako-api",
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let p = auth.resolve(&token).await.unwrap();
        assert_eq!(p.tenant_id, "acme");
    }

    #[tokio::test]
    async fn hs256_alg_confusion_rejected() {
        // A token signed with HS256 must not validate against an
        // RS256-configured resolver — the `alg` field is pinned at
        // construction so downgrade attacks fail closed.
        let secret = b"this-is-a-test-secret-that-is-long-enough";
        let claims = json!({
            "tenant_id": "acme",
            "sub": "alice",
            "exp": now_plus_secs(60),
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        // RSA public key (test fixture from the `jsonwebtoken` README).
        let rsa_pem = b"-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAtqgRRr6f3tWs5RpEYE3D\n\
JzeS0pgB68OebgQzNKxg5sCpOhPDszsxapRaRoUUgxhGawY2u4XaGzeYEhNXBzKm\n\
XQvlvsq7qfBIRGoanV9LwpKAcDB99cQUDR5o6YfhJTSE1KdmodRKfvuZ8AiFDWss\n\
Vbs+ohgnQMXh5O2A0TczgD/I+lRR+a9hbWFA/WwG4xGGUXxmtJEXBHgsSp2/h+oU\n\
hwRUkwCC7eUZNI4y84Z+iRkjUFQS+cOfSUJiE0EnbORS9HxzlfROrG5+u1KYRdOZ\n\
nNblxEzXtaRJsiQEJyV8DdBSt4ddQ+kuQ06bpwDNEGcL2eLqPM4KTcq5EAmCGd+E\n\
NQIDAQAB\n\
-----END PUBLIC KEY-----\n";
        let auth = JwtAuthResolver::rs256_from_pem(rsa_pem).unwrap();
        let err = auth.resolve(&token).await.unwrap_err();
        assert!(matches!(err, TakoError::Invalid(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn missing_tenant_claim_is_rejected() {
        let secret = b"this-is-a-test-secret-that-is-long-enough";
        let auth = JwtAuthResolver::hs256(secret);

        let claims = json!({
            "sub": "alice",
            "exp": now_plus_secs(60),
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let err = auth.resolve(&token).await.unwrap_err();
        assert!(matches!(err, TakoError::Invalid(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn with_claims_uses_overridden_claim_names() {
        let secret = b"this-is-a-test-secret-that-is-long-enough";
        let auth = JwtAuthResolver::hs256(secret).with_claims("tenant", "uid", "groups");

        let claims = json!({
            "tenant": "acme",
            "uid": "alice",
            "groups": ["admin"],
            "exp": now_plus_secs(60),
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let p = auth.resolve(&token).await.unwrap();
        assert_eq!(p.tenant_id, "acme");
        assert_eq!(p.user_id, "alice");
        assert_eq!(p.roles, vec!["admin".to_string()]);
    }
}
