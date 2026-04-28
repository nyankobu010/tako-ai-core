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

use crate::py_provider::ProviderHandle;

#[pyclass(name = "Orchestrator", module = "tako._native", skip_from_py_object)]
pub struct PyOrchestrator {
    inner: Arc<SingleAgent>,
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
    /// `provider` may be a `PyOpenAI`, `PyAnthropic`, or `PyFakeProvider`.
    #[new]
    #[pyo3(signature = (provider, max_steps=8))]
    fn new(provider: PyObject, max_steps: u32, py: Python<'_>) -> PyResult<Self> {
        let handle: ProviderHandle =
            if let Ok(p) = provider.extract::<crate::py_provider::PyOpenAI>(py) {
                p.handle
            } else if let Ok(p) = provider.extract::<crate::py_provider::PyAnthropic>(py) {
                p.handle
            } else if let Ok(p) = provider.extract::<crate::py_provider::PyFakeProvider>(py) {
                p.handle
            } else {
                return Err(PyValueError::new_err(
                    "provider must be a tako._native.OpenAI, Anthropic, or FakeProvider",
                ));
            };

        let agent = SingleAgent::builder()
            .provider(handle.inner)
            .tools(Arc::new(ToolRegistry::new()))
            .max_steps(max_steps)
            .build()
            .map_err(crate::py_provider::map_err)?;
        Ok(Self {
            inner: Arc::new(agent),
        })
    }

    /// Async run; returns a Python coroutine that resolves to a string.
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
            Ok(out.text)
        })
    }

    /// Synchronous run. Releases the GIL while blocking on the runtime so
    /// concurrent threads can hold it.
    #[pyo3(signature = (prompt, tenant_id=None, user_id=None))]
    fn run_sync(
        &self,
        py: Python<'_>,
        prompt: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
    ) -> PyResult<String> {
        let agent = Arc::clone(&self.inner);
        let principal = crate::conv::principal_from(tenant_id.as_deref(), user_id.as_deref());
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let out = py.detach(move || {
            rt.block_on(async move { agent.run(&principal, OrchInput::from_user(prompt)).await })
        });
        let out = out.map_err(crate::py_provider::map_err)?;
        Ok(out.text)
    }
}
