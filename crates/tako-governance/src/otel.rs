//! OpenTelemetry tracing pipeline.
//!
//! Phase 1 ships a minimal `init_tracing` that wires `tracing-subscriber`
//! with optional JSON output and an env-filter. Full OTLP exporter
//! integration with batching, gRPC, and resource attributes lands in
//! Phase 2 once we lock in the `opentelemetry-otlp` 0.31 surface against
//! the GenAI semantic-convention spans the orchestrator emits.

use serde::{Deserialize, Serialize};
use tako_core::TakoError;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Tracing configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TracingConfig {
    /// Env filter directive, e.g. `"tako=debug,info"`. Falls back to
    /// `RUST_LOG` if `None`.
    pub filter: Option<String>,
    /// Emit JSON-formatted log lines (recommended for production).
    #[serde(default)]
    pub json: bool,
    /// Future: OTLP exporter endpoint, e.g. `"http://localhost:4317"`.
    /// Phase 1 ignores this; logs only go to stderr. Set in your config
    /// today and Phase 2 will pick it up automatically.
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
}

/// Initialise tracing for the process. Idempotent: calling twice will
/// surface a `TakoError::Invalid` rather than panic.
pub fn init_tracing(config: &TracingConfig) -> Result<(), TakoError> {
    let filter = match &config.filter {
        Some(directive) => EnvFilter::new(directive),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    };

    let registry = tracing_subscriber::registry().with(filter);
    let result = if config.json {
        registry.with(fmt::layer().json()).try_init()
    } else {
        registry.with(fmt::layer()).try_init()
    };

    result.map_err(|e| TakoError::Invalid(format!("init_tracing: {e}")))
}

/// Phase-2 placeholder — wires up an OTLP exporter against
/// `config.otlp_endpoint`. Currently delegates to [`init_tracing`] so user
/// code can be written against the eventual API now.
pub fn init_otlp_tracing(config: &TracingConfig) -> Result<(), TakoError> {
    if let Some(endpoint) = &config.otlp_endpoint {
        tracing::warn!(endpoint, "OTLP exporter is Phase 2; falling back to local subscriber");
    }
    init_tracing(config)
}
