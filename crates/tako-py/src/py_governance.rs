//! Bindings for governance plumbing — Budget value type and tracing init.

use std::collections::HashMap;

use pyo3::prelude::*;
use tako_core::Budget;
use tako_governance::{TracingConfig, init_tracing};

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
