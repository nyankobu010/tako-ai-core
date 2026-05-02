//! Bindings for governance plumbing — Budget value type and tracing init.

use std::collections::HashMap;
use std::sync::Mutex;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_core::Budget;
use tako_governance::{TracerGuard, TracingConfig, init_otlp_tracing, init_tracing};

#[pyclass(name = "Budget", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyBudget {
    pub inner: Budget,
}

#[pymethods]
impl PyBudget {
    #[new]
    #[pyo3(signature = (max_usd_per_request=None, max_usd_per_day=None, max_tokens_per_request=None, max_usd_per_tenant_per_day=None))]
    fn new(
        max_usd_per_request: Option<f64>,
        max_usd_per_day: Option<f64>,
        max_tokens_per_request: Option<u32>,
        max_usd_per_tenant_per_day: Option<HashMap<String, f64>>,
    ) -> Self {
        let inner = Budget {
            max_usd_per_request,
            max_usd_per_day,
            max_tokens_per_request,
            max_usd_per_tenant_per_day: max_usd_per_tenant_per_day
                .unwrap_or_default()
                .into_iter()
                .collect(),
        };
        Self { inner }
    }

    fn __repr__(&self) -> String {
        format!("Budget({:?})", self.inner)
    }
}

/// Initialise tracing for the process. Idempotent — second call is a
/// no-op.
#[pyfunction]
#[pyo3(signature = (filter=None, json=false))]
pub fn init_tracing_py(filter: Option<String>, json: bool) -> PyResult<()> {
    let cfg = TracingConfig {
        filter,
        json,
        otlp_endpoint: None,
    };
    // Idempotent: ignore "already initialised" failures.
    let _ = init_tracing(&cfg);
    Ok(())
}

/// Process-global guard: keeps the OTLP tracer provider alive for the
/// lifetime of the Python interpreter. Only one OTLP exporter can be
/// active at a time (the tracing subscriber is process-wide).
static OTLP_GUARD: Mutex<Option<TracerGuard>> = Mutex::new(None);

/// Initialise tracing **with** an OTLP gRPC exporter. Pins the resulting
/// `TracerGuard` to a process-global so spans flush on interpreter exit.
///
/// Calling twice without an intervening shutdown is rejected.
///
/// The OTLP exporter (via tonic + hyper) requires a Tokio reactor in
/// scope at construction time, so we enter the shared pyo3-async-runtimes
/// runtime handle for the duration of the build.
#[pyfunction]
#[pyo3(signature = (endpoint, filter=None, json=false))]
pub fn init_otlp_tracing_py(endpoint: String, filter: Option<String>, json: bool) -> PyResult<()> {
    let mut slot = OTLP_GUARD
        .lock()
        .map_err(|e| PyValueError::new_err(format!("OTLP guard mutex poisoned: {e}")))?;
    if slot.is_some() {
        return Err(PyValueError::new_err(
            "OTLP tracing already initialised; call shutdown_otlp_py() first",
        ));
    }
    let cfg = TracingConfig {
        filter,
        json,
        otlp_endpoint: Some(endpoint),
    };
    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let _enter = rt.handle().enter();
    let guard = init_otlp_tracing(&cfg).map_err(|e| PyValueError::new_err(e.to_string()))?;
    *slot = Some(guard);
    Ok(())
}

/// Drop the OTLP guard, flushing pending spans. Idempotent.
#[pyfunction]
pub fn shutdown_otlp_py() -> PyResult<()> {
    let mut slot = OTLP_GUARD
        .lock()
        .map_err(|e| PyValueError::new_err(format!("OTLP guard mutex poisoned: {e}")))?;
    if let Some(guard) = slot.take() {
        guard.shutdown();
    }
    Ok(())
}
