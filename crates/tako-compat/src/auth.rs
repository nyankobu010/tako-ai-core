//! Bearer-token auth — the OpenAI SDK sends `Authorization: Bearer <token>`.
//!
//! The compat server resolves the token to a [`tako_core::Principal`] via
//! a pluggable [`AuthResolver`]. Production deployments typically swap
//! [`StaticTokens`] for a real provider (Vault, JWT, OIDC), but Phase 2
//! ships only the static map.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tako_core::{Principal, TakoError};

/// Resolves a bearer token to the calling principal.
#[async_trait]
pub trait AuthResolver: Send + Sync + 'static + std::fmt::Debug {
    async fn resolve(&self, token: &str) -> Result<Principal, TakoError>;
}

/// In-memory token table. Tokens map to `(tenant_id, user_id, roles)`.
#[derive(Clone, Debug, Default)]
pub struct StaticTokens {
    table: Arc<HashMap<String, Principal>>,
}

impl StaticTokens {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, token: impl Into<String>, principal: Principal) -> Self {
        let mut t = (*self.table).clone();
        t.insert(token.into(), principal);
        self.table = Arc::new(t);
        self
    }

    pub fn from_map(map: HashMap<String, Principal>) -> Self {
        Self {
            table: Arc::new(map),
        }
    }
}

#[async_trait]
impl AuthResolver for StaticTokens {
    async fn resolve(&self, token: &str) -> Result<Principal, TakoError> {
        self.table
            .get(token)
            .cloned()
            .ok_or_else(|| TakoError::Invalid("unknown bearer token".into()))
    }
}
