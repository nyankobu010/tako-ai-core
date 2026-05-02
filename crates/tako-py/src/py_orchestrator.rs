//! `PyOrchestrator` — wraps `tako-orchestrator::SingleAgent`.

use std::sync::Arc;

use pyo3::Py;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyAny;

type PyObject = Py<PyAny>;
use pyo3_async_runtimes::tokio::future_into_py;
use tako_mcp::ToolRegistry;
use tako_orchestrator::{OrchInput, Orchestrator, SingleAgent};
use tako_runtime::BudgetTracker;

use crate::py_governance::PyBudget;
use crate::py_provider::ProviderHandle;

#[pyclass(name = "Orchestrator", module = "tako._native", skip_from_py_object)]
pub struct PyOrchestrator {
    pub(crate) inner: Arc<SingleAgent>,
}

impl PyOrchestrator {
    /// Return the orchestrator handle as `Arc<dyn Orchestrator>` for
    /// downstream wiring (compat server, future routing).
    pub(crate) fn inner_arc(&self) -> Arc<dyn Orchestrator> {
        Arc::clone(&self.inner) as Arc<dyn Orchestrator>
    }
}

impl std::fmt::Debug for PyOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyOrchestrator").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyOrchestrator {
    /// Build a single-agent orchestrator wrapping a provider.
    ///
    /// `provider` may be a `PyOpenAI`, `PyAnthropic`, `PyAzureOpenAi`,
    /// `PyBedrock`, `PyFakeProvider`, or `PyPythonProvider`.
    /// `mcp_servers` is an optional list of `PyStdio` / `PyStreamableHttp`
    /// transports; their tools are discovered at construction time and
    /// merged into the orchestrator's tool registry.
    #[new]
    #[pyo3(signature = (
        provider,
        max_steps=8,
        mcp_servers=None,
        candidates=None,
        router=None,
        budget=None,
        budget_backend=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        provider: PyObject,
        max_steps: u32,
        mcp_servers: Option<Vec<PyObject>>,
        candidates: Option<Vec<PyObject>>,
        router: Option<PyObject>,
        budget: Option<PyBudget>,
        budget_backend: Option<PyObject>,
        py: Python<'_>,
    ) -> PyResult<Self> {
        let handle: ProviderHandle = if let Ok(p) =
            provider.extract::<crate::py_provider::PyOpenAI>(py)
        {
            p.handle
        } else if let Ok(p) = provider.extract::<crate::py_provider::PyAnthropic>(py) {
            p.handle
        } else if let Ok(p) = provider.extract::<crate::py_provider::PyFakeProvider>(py) {
            p.handle
        } else if let Ok(p) = provider.extract::<crate::py_azure::PyAzureOpenAi>(py) {
            p.handle
        } else if let Ok(p) = provider.extract::<crate::py_bedrock::PyBedrock>(py) {
            p.handle
        } else if let Ok(p) = provider.extract::<crate::py_vertex::PyVertex>(py) {
            p.handle
        } else if let Ok(p) = provider.extract::<crate::py_http_generic::PyHttpGeneric>(py) {
            p.handle
        } else if let Ok(p) = provider.extract::<crate::py_python_provider::PyPythonProvider>(py) {
            p.handle
        } else {
            return Err(PyValueError::new_err(
                "provider must be a tako._native.OpenAI, Anthropic, AzureOpenAi, Bedrock, Vertex, HttpGeneric, FakeProvider, or PythonProvider",
            ));
        };

        let registry = Arc::new(ToolRegistry::new());

        if let Some(servers) = mcp_servers {
            let rt = pyo3_async_runtimes::tokio::get_runtime();
            let handles: Result<Vec<_>, PyErr> = servers
                .iter()
                .map(|s| crate::py_mcp::extract_transport_handle(py, s))
                .collect();
            let handles = handles?;
            let registry_clone = Arc::clone(&registry);
            let result: Result<(), tako_core::TakoError> = py.detach(move || {
                rt.block_on(async move {
                    for h in handles {
                        registry_clone.discover(h.inner).await?;
                    }
                    Ok(())
                })
            });
            result.map_err(crate::py_provider::map_err)?;
        }

        let mut builder = SingleAgent::builder()
            .provider(handle.inner)
            .tools(registry)
            .max_steps(max_steps);
        if let Some(extra) = candidates {
            for c in extra {
                let cand = crate::py_conductor::extract_any_provider(py, &c)?;
                builder = builder.candidate(cand);
            }
        }
        if let Some(r) = router {
            let router_arc = crate::py_router::extract_router(py, &r)?;
            builder = builder.router(router_arc);
        }
        if budget.is_some() || budget_backend.is_some() {
            let budget_inner = budget.map(|b| b.inner).unwrap_or_default();
            let backend = if let Some(obj) = budget_backend {
                crate::py_runtime::extract_budget_backend(py, &obj)?
            } else {
                // Default to in-memory when only `budget=` is given.
                Arc::new(tako_runtime::InMemoryBudgetBackend::new())
                    as Arc<dyn tako_runtime::BudgetBackend>
            };
            let tracker = Arc::new(BudgetTracker::new(backend, budget_inner));
            builder = builder.budget(tracker);
        }
        let agent = builder.build().map_err(crate::py_provider::map_err)?;
        Ok(Self {
            inner: Arc::new(agent),
        })
    }

    /// Async run; returns a Python coroutine that resolves to an
    /// `OrchOutput` (Phase 46.B). `result.text` is unchanged from the
    /// pre-46 string return; new fields `input_tokens`,
    /// `output_tokens`, `total_tokens`, and `steps` are additive.
    #[pyo3(signature = (prompt, tenant_id=None, user_id=None))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        prompt: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let agent = Arc::clone(&self.inner);
        let principal = crate::conv::principal_from(tenant_id.as_deref(), user_id.as_deref());
        future_into_py(py, async move {
            let out = agent
                .run(&principal, OrchInput::from_user(prompt))
                .await
                .map_err(|e| PyValueError::new_err(e.to_string()))?;
            Ok(crate::py_orch_output::PyOrchOutput::new(out))
        })
    }

    /// Synchronous run. Releases the GIL while blocking on the runtime so
    /// concurrent threads can hold it. Returns an `OrchOutput`
    /// (Phase 46.B).
    #[pyo3(signature = (prompt, tenant_id=None, user_id=None))]
    fn run_sync(
        &self,
        py: Python<'_>,
        prompt: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
    ) -> PyResult<crate::py_orch_output::PyOrchOutput> {
        let agent = Arc::clone(&self.inner);
        let principal = crate::conv::principal_from(tenant_id.as_deref(), user_id.as_deref());
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let out = py.detach(move || {
            rt.block_on(async move { agent.run(&principal, OrchInput::from_user(prompt)).await })
        });
        let out = out.map_err(crate::py_provider::map_err)?;
        Ok(crate::py_orch_output::PyOrchOutput::new(out))
    }
}
