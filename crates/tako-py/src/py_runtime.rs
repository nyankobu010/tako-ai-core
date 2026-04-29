//! Bindings for `tako-runtime` extras that don't fit elsewhere.
//!
//! Phase 4.G adds [`PyRedisBudgetBackend`], a thin wrapper over
//! [`tako_runtime::RedisBudgetBackend`]. Gated behind the `redis` Cargo
//! feature; the underlying crate dep arrives via `tako-runtime/redis`
//! only when this feature is enabled.
#![cfg(feature = "redis")]

use std::sync::Arc;
use std::time::Duration;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_core::TakoError;
use tako_runtime::{BudgetBackend, RedisBudgetBackend};

use crate::py_provider::map_err;

/// Redis-backed `BudgetBackend` for multi-process deployments.
///
/// Construct with `RedisBudgetBackend(url, key_prefix=None,
/// ttl_secs=None)` and call the async `.current_usage(tenant_id)` /
/// `.record(tenant_id, usd, tokens)` methods. Both return Python
/// awaitables suitable for `await`.
///
/// The constructor blocks the calling Python thread until the initial
/// connection completes; the GIL is released for the blocking section.
#[pyclass(name = "RedisBudgetBackend", module = "tako._native")]
pub struct PyRedisBudgetBackend {
    inner: Arc<RedisBudgetBackend>,
    url: String,
}

impl std::fmt::Debug for PyRedisBudgetBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyRedisBudgetBackend")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

#[pymethods]
impl PyRedisBudgetBackend {
    #[new]
    #[pyo3(signature = (url, key_prefix=None, ttl_secs=None))]
    fn new(
        py: Python<'_>,
        url: String,
        key_prefix: Option<String>,
        ttl_secs: Option<u64>,
    ) -> PyResult<Self> {
        let url_clone = url.clone();
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let backend = py.detach(|| {
            rt.block_on(async move {
                let mut b = RedisBudgetBackend::connect(&url_clone).await?;
                if let Some(p) = key_prefix {
                    b = b.with_key_prefix(p);
                }
                if let Some(t) = ttl_secs {
                    b = b.with_ttl(Duration::from_secs(t));
                }
                Ok::<_, TakoError>(b)
            })
        });
        let backend = backend.map_err(map_err)?;
        Ok(Self {
            inner: Arc::new(backend),
            url,
        })
    }

    /// Return `(usd_today, tokens_today)` for `tenant_id`. Awaitable.
    fn current_usage<'py>(
        &self,
        py: Python<'py>,
        tenant_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let backend = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let usage = backend
                .current_usage(&tenant_id)
                .await
                .map_err(|e: TakoError| PyValueError::new_err(e.to_string()))?;
            Ok((usage.usd_today, usage.tokens_today))
        })
    }

    /// Record `usd` + `tokens` against `tenant_id`. Awaitable.
    fn record<'py>(
        &self,
        py: Python<'py>,
        tenant_id: String,
        usd: f64,
        tokens: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let backend = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            backend
                .record(&tenant_id, usd, tokens)
                .await
                .map_err(|e: TakoError| PyValueError::new_err(e.to_string()))?;
            Ok(())
        })
    }

    fn __repr__(&self) -> String {
        format!("RedisBudgetBackend(url={:?})", self.url)
    }
}
