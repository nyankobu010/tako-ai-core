//! `tako-governance` — enterprise plumbing for tako.
//!
//! Phase 1 ships:
//! - [`otel::init_tracing`] — minimal tracing-subscriber wiring.
//! - [`pii`] — Presidio-style regex + Luhn detection with `Mask` /
//!   `HashSha256` / `Redact` transforms.
//! - [`secrets::EnvResolver`] + [`secrets::SecretString`] redacting
//!   wrapper.
//!
//! OPA/regorus enforcement and cloud-vendor secret resolvers arrive in
//! Phase 2.

#[cfg(feature = "sigstore-protobuf")]
pub(crate) mod cosign_bundle;
pub mod otel;
pub mod pii;
pub mod policy;
pub mod secrets;
#[cfg(feature = "sigstore")]
pub mod sigstore;
#[cfg(feature = "sigstore")]
pub mod sigstore_state;

pub use otel::{TracerGuard, TracingConfig, init_otlp_tracing, init_tracing};
pub use pii::{ContentTransform, PiiHit, PiiKind, apply, detect};
pub use policy::{AuditLog, OpaBundle};
pub use secrets::{
    AwsSecretsManagerResolver, AzureKeyVaultResolver, EnvResolver, GcpSecretManagerResolver,
    SecretResolver, SecretString, VaultResolver,
};
#[cfg(feature = "sigstore")]
pub use sigstore::{
    Catalogue, CatalogueVerifier, IdentityPolicy, KeylessBundle, KeylessVerifier, SanMatch,
};
#[cfg(feature = "sigstore")]
pub use sigstore_state::JsonStateStore;
