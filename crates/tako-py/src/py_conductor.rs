//! `PyConductor` — wraps `tako_orchestrator::Conductor`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;
use tako_core::LlmProvider;
use tako_orchestrator::{Conductor, OrchInput, Orchestrator};
use tako_runtime::BudgetTracker;

use crate::py_governance::PyBudget;
use crate::py_provider::{ProviderHandle, map_err};

#[pyclass(name = "Conductor", module = "tako._native", skip_from_py_object)]
pub struct PyConductor {
    pub(crate) inner: Arc<Conductor>,
}

impl PyConductor {
    pub(crate) fn inner_arc(&self) -> Arc<dyn Orchestrator> {
        Arc::clone(&self.inner) as Arc<dyn Orchestrator>
    }
}

impl std::fmt::Debug for PyConductor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyConductor").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyConductor {
    /// Build a Conductor.
    ///
    /// `coordinator` and each entry in `workers` (a dict
    /// `{role: provider}`) accept the usual provider classes
    /// (OpenAI, Anthropic, Bedrock, FakeProvider, PythonProvider).
    ///
    /// `verifier` (Phase 10.C) attaches an optional
    /// `tako._native.RuleBasedVerifier`. When set, the streaming
    /// path emits one `OrchEvent::VerifierScore` per worker output
    /// before fold-in, with `branch` = the 1-based worker dispatch
    /// index within the current step. Without this kwarg, no
    /// `VerifierScore` events appear.
    #[new]
    #[pyo3(signature = (
        coordinator,
        workers,
        max_steps=6,
        max_fanout=4,
        worker_timeout_secs=120,
        fail_fast=false,
        budget=None,
        budget_backend=None,
        verifier=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        coordinator: Py<PyAny>,
        workers: HashMap<String, Py<PyAny>>,
        max_steps: u32,
        max_fanout: usize,
        worker_timeout_secs: u64,
        fail_fast: bool,
        budget: Option<PyBudget>,
        budget_backend: Option<Py<PyAny>>,
        verifier: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let coord_handle = extract_provider(py, &coordinator)?;
        let mut builder = Conductor::builder()
            .coordinator(coord_handle.inner)
            .max_steps(max_steps)
            .max_fanout(max_fanout)
            .worker_timeout(Duration::from_secs(worker_timeout_secs))
            .fail_fast(fail_fast);
        for (name, w) in workers {
            let h = extract_provider(py, &w)?;
            builder = builder.worker(name, h.inner);
        }
        if budget.is_some() || budget_backend.is_some() {
            let budget_inner = budget.map(|b| b.inner).unwrap_or_default();
            let backend = if let Some(obj) = budget_backend {
                crate::py_runtime::extract_budget_backend(py, &obj)?
            } else {
                Arc::new(tako_runtime::InMemoryBudgetBackend::new())
                    as Arc<dyn tako_runtime::BudgetBackend>
            };
            let tracker = Arc::new(BudgetTracker::new(backend, budget_inner));
            builder = builder.budget(tracker);
        }
        if let Some(obj) = verifier {
            let v = crate::py_ab_mcts::extract_any_verifier(py, &obj)?;
            builder = builder.verifier(v);
        }
        let cond = builder.build().map_err(map_err)?;
        Ok(Self {
            inner: Arc::new(cond),
        })
    }

    /// Async run.
    #[pyo3(signature = (prompt, tenant_id=None, user_id=None))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        prompt: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let cond = Arc::clone(&self.inner);
        let principal = crate::conv::principal_from(tenant_id.as_deref(), user_id.as_deref());
        future_into_py(py, async move {
            let out = cond
                .run(&principal, OrchInput::from_user(prompt))
                .await
                .map_err(|e| PyValueError::new_err(e.to_string()))?;
            Ok(out.text)
        })
    }

    #[pyo3(signature = (prompt, tenant_id=None, user_id=None))]
    fn run_sync(
        &self,
        py: Python<'_>,
        prompt: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
    ) -> PyResult<String> {
        let cond = Arc::clone(&self.inner);
        let principal = crate::conv::principal_from(tenant_id.as_deref(), user_id.as_deref());
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let out = py.detach(move || {
            rt.block_on(async move { cond.run(&principal, OrchInput::from_user(prompt)).await })
        });
        let out = out.map_err(map_err)?;
        Ok(out.text)
    }
}

fn extract_provider(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<ProviderHandle> {
    if let Ok(p) = obj.extract::<crate::py_provider::PyOpenAI>(py) {
        return Ok(p.handle);
    }
    if let Ok(p) = obj.extract::<crate::py_provider::PyAnthropic>(py) {
        return Ok(p.handle);
    }
    if let Ok(p) = obj.extract::<crate::py_provider::PyFakeProvider>(py) {
        return Ok(p.handle);
    }
    if let Ok(p) = obj.extract::<crate::py_azure::PyAzureOpenAi>(py) {
        return Ok(p.handle);
    }
    if let Ok(p) = obj.extract::<crate::py_bedrock::PyBedrock>(py) {
        return Ok(p.handle);
    }
    if let Ok(p) = obj.extract::<crate::py_vertex::PyVertex>(py) {
        return Ok(p.handle);
    }
    if let Ok(p) = obj.extract::<crate::py_http_generic::PyHttpGeneric>(py) {
        return Ok(p.handle);
    }
    if let Ok(p) = obj.extract::<crate::py_python_provider::PyPythonProvider>(py) {
        return Ok(p.handle);
    }
    Err(PyValueError::new_err(
        "provider must be a tako._native.OpenAI, Anthropic, AzureOpenAi, Bedrock, Vertex, HttpGeneric, FakeProvider, or PythonProvider",
    ))
}

/// Cast a Python provider object to its underlying `LlmProvider` Arc.
/// Used by other PyO3 modules that need a heterogeneous "any provider"
/// extract; not a public method.
#[allow(dead_code)]
pub(crate) fn extract_any_provider(
    py: Python<'_>,
    obj: &Py<PyAny>,
) -> PyResult<Arc<dyn LlmProvider>> {
    extract_provider(py, obj).map(|h| h.inner)
}
