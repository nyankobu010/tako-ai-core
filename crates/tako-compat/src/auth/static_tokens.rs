//! `StaticTokens` — in-memory `token → Principal` table.
//!
//! Phase 2 baseline. Suitable for dev, CI, and small deployments where
//! the token list is curated by hand. Production deployments swap this
//! for [`JwtAuthResolver`](super::JwtAuthResolver),
//! [`OidcAuthResolver`](super::OidcAuthResolver), or
//! [`VaultAuthResolver`](super::VaultAuthResolver) (Phase 14.B).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tako_core::{Principal, TakoError};

use super::AuthResolver;

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
