//! OpenTelemetry tracing pipeline.
//!
//! Two entry points:
//!
//! - [`init_tracing`] wires `tracing-subscriber` for stderr-only logs (with
//!   optional JSON layout); idempotent.
//! - [`init_otlp_tracing`] additionally attaches an OTLP gRPC exporter (via
//!   `opentelemetry-otlp` 0.31 + `tonic`) so spans land in a remote
//!   collector. Returns a [`TracerGuard`] that flushes pending spans on
//!   drop.

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use serde::{Deserialize, Serialize};
use tako_core::TakoError;
use tracing_subscriber::{EnvFilter, Layer, fmt, prelude::*};

/// Tracing configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TracingConfig {
    /// Env filter directive, e.g. `"tako=debug,info"`. Falls back to
    /// `RUST_LOG` if `None`.
    pub filter: Option<String>,
    /// Emit JSON-formatted log lines (recommended for production).
    #[serde(default)]
    pub json: bool,
    /// Optional OTLP endpoint, e.g. `"http://localhost:4317"`. If set,
    /// [`init_otlp_tracing`] attaches a gRPC OTLP exporter.
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
}

/// Initialise tracing for the process. Idempotent: a second call is a
/// no-op (and surfaces a `TakoError::Invalid` with the underlying reason).
pub fn init_tracing(config: &TracingConfig) -> Result<(), TakoError> {
    let registry = tracing_subscriber::registry().with(filter_from(config));
    let result = if config.json {
        registry.with(fmt::layer().json()).try_init()
    } else {
        registry.with(fmt::layer()).try_init()
    };
    result.map_err(|e| TakoError::Invalid(format!("init_tracing: {e}")))
}

/// Initialise tracing **with** an OTLP gRPC exporter.
///
/// Builds an [`opentelemetry_otlp::SpanExporter`] over `tonic`, wraps it in
/// a [`SdkTracerProvider`] with a batch processor, and attaches it to
/// `tracing-subscriber` via [`tracing_opentelemetry::layer`].
///
/// Returns a [`TracerGuard`] whose `Drop` impl shuts the provider down so
/// pending spans are flushed before the process exits. Drop the guard
/// (or let it run to end-of-`main`) — don't `mem::forget` it.
///
/// Idempotent: a second call returns `TakoError::Invalid` rather than
/// panicking.
pub fn init_otlp_tracing(config: &TracingConfig) -> Result<TracerGuard, TakoError> {
    let endpoint = config
        .otlp_endpoint
        .as_deref()
        .ok_or_else(|| TakoError::Invalid("init_otlp_tracing: otlp_endpoint is required".into()))?;

    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| TakoError::Transport(format!("OTLP exporter build: {e}")))?;

    let resource = Resource::builder()
        .with_service_name("tako")
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    let tracer = provider.tracer("tako");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let fmt_layer: Box<dyn Layer<_> + Send + Sync> = if config.json {
        Box::new(fmt::layer().json())
    } else {
        Box::new(fmt::layer())
    };

    tracing_subscriber::registry()
        .with(filter_from(config))
        .with(otel_layer)
        .with(fmt_layer)
        .try_init()
        .map_err(|e| TakoError::Invalid(format!("init_otlp_tracing: {e}")))?;

    Ok(TracerGuard {
        provider: Some(provider),
    })
}

fn filter_from(config: &TracingConfig) -> EnvFilter {
    match &config.filter {
        Some(directive) => EnvFilter::new(directive),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    }
}

/// Holds the [`SdkTracerProvider`] for the lifetime of OTLP-instrumented
/// code. On drop, calls `shutdown()` to flush the batch processor.
///
/// Only one of these can be live at a time (the global tracing subscriber
/// is process-wide). Don't `mem::forget` it.
pub struct TracerGuard {
    provider: Option<SdkTracerProvider>,
}

impl std::fmt::Debug for TracerGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TracerGuard").finish_non_exhaustive()
    }
}

impl TracerGuard {
    /// Explicitly shut down the provider. Subsequent calls are no-ops.
    pub fn shutdown(mut self) {
        if let Some(p) = self.provider.take() {
            let _ = p.shutdown();
        }
    }
}

impl Drop for TracerGuard {
    fn drop(&mut self) {
        if let Some(p) = self.provider.take() {
            let _ = p.shutdown();
        }
    }
}
