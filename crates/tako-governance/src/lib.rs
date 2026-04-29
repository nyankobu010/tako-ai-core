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

pub mod otel;
pub mod pii;
pub mod policy;
pub mod secrets;

pub use otel::{TracerGuard, TracingConfig, init_otlp_tracing, init_tracing};
pub use pii::{ContentTransform, PiiHit, PiiKind, apply, detect};
pub use policy::{AuditLog, OpaBundle};
pub use secrets::{EnvResolver, SecretResolver, SecretString};
