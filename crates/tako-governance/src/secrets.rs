//! `SecretResolver` trait + `EnvResolver` impl + redacting `SecretString`.
//!
//! Vault / AWS SM / Azure KV / GCP SM resolvers arrive in Phase 2.

use std::env;

use async_trait::async_trait;
use tako_core::TakoError;

/// A string we never want to print. `Debug` and `Display` both render
/// `<redacted>`; the underlying value is accessible only via [`expose`].
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// Read the underlying value. Use sparingly and never log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("\"<redacted>\"")
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Resolves a secret name to its value. Implementations may pull from
/// environment variables, files, Vault, cloud secret stores, etc.
#[async_trait]
pub trait SecretResolver: Send + Sync + 'static + std::fmt::Debug {
    async fn resolve(&self, key: &str) -> Result<SecretString, TakoError>;
}

/// Reads from the process environment.
#[derive(Debug, Default, Clone)]
pub struct EnvResolver;

#[async_trait]
impl SecretResolver for EnvResolver {
    async fn resolve(&self, key: &str) -> Result<SecretString, TakoError> {
        env::var(key)
            .map(SecretString::new)
            .map_err(|_| TakoError::NotFound(format!("env var `{key}` is not set")))
    }
}
