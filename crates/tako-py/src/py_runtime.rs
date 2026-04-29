//! Bindings for `tako-runtime` extras.
//!
//! Phase 4.G added [`PyRedisBudgetBackend`] (gated on `redis`).
//! Phase 5.C adds [`PyInMemoryBudgetBackend`] — always available — so
//! orchestrators can be wired with budget enforcement without requiring
//! a Redis backend.

use std::sync::Arc;
#[cfg(feature = "redis")]
use std::time::Duration;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use tako_core::TakoError;
#[cfg(feature = "redis")]
use tako_runtime::RedisBudgetBackend;
use tako_runtime::{BudgetBackend, InMemoryBudgetBackend};

#[cfg(feature = "redis")]
use crate::py_provider::map_err;

/// In-memory `BudgetBackend` — single-process, no day-rollover (the
/// in-memory state is reset only when the process restarts). Suitable
/// for local development and tests; production deployments should use
/// the Redis backend.
#[pyclass(name = "InMemoryBudgetBackend", module = "tako._native")]
#[derive(Debug)]
pub struct PyInMemoryBudgetBackend {
    inner: Arc<InMemoryBudgetBackend>,
}

#[pymethods]
impl PyInMemoryBudgetBackend {
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(InMemoryBudgetBackend::new()),
        }
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
        "InMemoryBudgetBackend()".to_string()
    }
}

impl PyInMemoryBudgetBackend {
    pub(crate) fn handle(&self) -> Arc<dyn BudgetBackend> {
        Arc::clone(&self.inner) as Arc<dyn BudgetBackend>
    }
}

/// Redis-backed `BudgetBackend` for multi-process deployments.
///
/// Construct with `RedisBudgetBackend(url, key_prefix=None,
/// ttl_secs=None)` and call the async `.current_usage(tenant_id)` /
/// `.record(tenant_id, usd, tokens)` methods. Both return Python
/// awaitables suitable for `await`.
///
/// The constructor blocks the calling Python thread until the initial
/// connection completes; the GIL is released for the blocking section.
#[cfg(feature = "redis")]
#[pyclass(name = "RedisBudgetBackend", module = "tako._native")]
pub struct PyRedisBudgetBackend {
    inner: Arc<RedisBudgetBackend>,
    url: String,
}

#[cfg(feature = "redis")]
impl std::fmt::Debug for PyRedisBudgetBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyRedisBudgetBackend")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "redis")]
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

#[cfg(feature = "redis")]
impl PyRedisBudgetBackend {
    pub(crate) fn handle(&self) -> Arc<dyn BudgetBackend> {
        Arc::clone(&self.inner) as Arc<dyn BudgetBackend>
    }
}

/// Extract an `Arc<dyn BudgetBackend>` from either of the two pyclass
/// wrappers. Used by the orchestrator constructors so callers can pass
/// `tako.budget.InMemoryBackend()` or `tako.budget.RedisBackend(...)`
/// interchangeably.
pub fn extract_budget_backend(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn BudgetBackend>> {
    if let Ok(b) = obj.extract::<PyRef<'_, PyInMemoryBudgetBackend>>(py) {
        return Ok(b.handle());
    }
    #[cfg(feature = "redis")]
    {
        if let Ok(b) = obj.extract::<PyRef<'_, PyRedisBudgetBackend>>(py) {
            return Ok(b.handle());
        }
    }
    Err(PyValueError::new_err(
        "budget_backend must be tako.budget.InMemoryBackend or tako.budget.RedisBackend",
    ))
}
