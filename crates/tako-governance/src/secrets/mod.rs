//! `SecretResolver` trait, the redacting `SecretString`, and resolver
//! implementations.
//!
//! Phase 1 shipped [`EnvResolver`]; Phase 2.5 adds cloud-vendor resolvers:
//! [`VaultResolver`], [`AwsSecretsManagerResolver`],
//! [`AzureKeyVaultResolver`], [`GcpSecretManagerResolver`].

use std::env;

use async_trait::async_trait;
use tako_core::TakoError;

mod aws_sm;
mod azure_kv;
mod gcp_sm;
mod vault;

pub use aws_sm::AwsSecretsManagerResolver;
pub use azure_kv::AzureKeyVaultResolver;
pub use gcp_sm::GcpSecretManagerResolver;
pub use vault::VaultResolver;

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
