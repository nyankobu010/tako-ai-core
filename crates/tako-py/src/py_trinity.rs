//! `PyTrinity` — wraps `tako_orchestrator::Trinity`.

use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;
use tako_orchestrator::{OrchInput, Orchestrator, Trinity};
use tako_runtime::BudgetTracker;

use crate::py_conductor::extract_any_provider;
use crate::py_governance::PyBudget;
use crate::py_router::extract_router;

#[pyclass(name = "Trinity", module = "tako._native", skip_from_py_object)]
pub struct PyTrinity {
    pub(crate) inner: Arc<Trinity>,
}

impl PyTrinity {
    pub(crate) fn inner_arc(&self) -> Arc<dyn Orchestrator> {
        Arc::clone(&self.inner) as Arc<dyn Orchestrator>
    }
}

impl std::fmt::Debug for PyTrinity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyTrinity").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyTrinity {
    /// Build a Trinity orchestrator.
    ///
    /// `roles` is a dict mapping `role_name -> provider`. `router` is one
    /// of `tako._native.RegexRouter` or `tako._native.OnnxRouter`.
    ///
    /// `verifier` (Phase 10.C) attaches an optional
    /// `tako._native.RuleBasedVerifier`. When set, the streaming path
    /// emits one `OrchEvent::VerifierScore` per role's assistant
    /// turn, with `branch` = the role's positional index in the
    /// insertion order. Without this kwarg, no `VerifierScore`
    /// events appear.
    #[new]
    #[pyo3(signature = (roles, router, max_steps=8, budget=None, budget_backend=None, verifier=None))]
    fn new(
        py: Python<'_>,
        roles: Vec<(String, Py<PyAny>)>,
        router: Py<PyAny>,
        max_steps: u32,
        budget: Option<PyBudget>,
        budget_backend: Option<Py<PyAny>>,
        verifier: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let mut builder = Trinity::builder()
            .router(extract_router(py, &router)?)
            .max_steps(max_steps);
        for (name, p) in roles {
            let prov = extract_any_provider(py, &p)?;
            builder = builder.role(name, prov);
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
        let trinity = builder
            .build()
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(trinity),
        })
    }

    #[pyo3(signature = (prompt, tenant_id=None, user_id=None))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        prompt: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let principal = crate::conv::principal_from(tenant_id.as_deref(), user_id.as_deref());
        future_into_py(py, async move {
            let out = inner
                .run(&principal, OrchInput::from_user(prompt))
                .await
                .map_err(|e| PyValueError::new_err(e.to_string()))?;
            Ok(crate::py_orch_output::PyOrchOutput::new(out))
        })
    }

    #[pyo3(signature = (prompt, tenant_id=None, user_id=None))]
    fn run_sync(
        &self,
        py: Python<'_>,
        prompt: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
    ) -> PyResult<crate::py_orch_output::PyOrchOutput> {
        let inner = Arc::clone(&self.inner);
        let principal = crate::conv::principal_from(tenant_id.as_deref(), user_id.as_deref());
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let out = py.detach(move || {
            rt.block_on(async move { inner.run(&principal, OrchInput::from_user(prompt)).await })
        });
        let out = out.map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(crate::py_orch_output::PyOrchOutput::new(out))
    }
}
