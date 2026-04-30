//! Bearer-token auth — the OpenAI SDK sends `Authorization: Bearer <token>`.
//!
//! The compat server resolves the token to a [`tako_core::Principal`] via
//! a pluggable [`AuthResolver`]. Each implementation lives in its own
//! sub-module:
//!
//! - [`StaticTokens`] (always on) — in-memory map for dev / CI.
//! - [`JwtAuthResolver`] (feature `jwt`) — verifies a signed JWT
//!   against a configured key (HS256 / RS256 / ES256).
//! - [`OidcAuthResolver`] (feature `oidc`) — discovers an OIDC
//!   provider's JWKS and validates incoming ID tokens against it.
//! - [`VaultAuthResolver`] (feature `vault`) — looks up bearer tokens
//!   in HashiCorp Vault KV v2.
//!
//! Phase 14.B adds the latter three; the trait and `StaticTokens` are
//! unchanged.

use async_trait::async_trait;
use tako_core::{Principal, TakoError};

mod static_tokens;
pub use static_tokens::StaticTokens;

#[cfg(feature = "jwt")]
mod jwt;
#[cfg(feature = "jwt")]
pub use jwt::JwtAuthResolver;

#[cfg(feature = "oidc")]
mod oidc;
#[cfg(feature = "oidc")]
pub use oidc::OidcAuthResolver;

#[cfg(feature = "vault")]
mod vault;
#[cfg(feature = "vault")]
pub use vault::VaultAuthResolver;

#[cfg(feature = "vault")]
mod vault_token;
#[cfg(feature = "vault")]
pub use vault_token::{
    AppRoleTokenProvider, DEFAULT_KUBERNETES_JWT_PATH, KubernetesTokenProvider, StaticVaultToken,
    VaultTokenProvider,
};

/// Resolves a bearer token to the calling principal.
#[async_trait]
pub trait AuthResolver: Send + Sync + 'static + std::fmt::Debug {
    async fn resolve(&self, token: &str) -> Result<Principal, TakoError>;
}
