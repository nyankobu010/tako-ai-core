//! Python entry points for the OpenAI-compatible HTTP server.
//!
//! The compat server boots in a background Tokio task; this module
//! exposes a `serve_openai_py(orch, host, port, tokens, models)`
//! function that:
//!
//! 1. Extracts the orchestrator handle from a PyOrchestrator or
//!    PyConductor (any tako._native orchestrator).
//! 2. Builds a StaticTokens auth resolver from a Python dict.
//! 3. Boots the server on the shared pyo3-async-runtimes Tokio runtime.
//! 4. Returns the bound URL string so the caller knows where it landed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_compat::{ServeConfig, StaticTokens, serve_openai};
use tako_core::Principal;
use tako_orchestrator::Orchestrator;

use crate::py_provider::map_err;

/// Process-global handle to the running compat server. We support a
/// single live server per process; calling serve again after shutdown
/// is fine.
static SERVER: OnceLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();

fn server_slot() -> &'static Mutex<Option<tokio::task::JoinHandle<()>>> {
    SERVER.get_or_init(|| Mutex::new(None))
}

#[pyfunction]
#[pyo3(signature = (orch, host="127.0.0.1", port=8080, tokens=None, models=None))]
pub fn serve_openai_py(
    py: Python<'_>,
    orch: Py<PyAny>,
    host: &str,
    port: u16,
    tokens: Option<HashMap<String, (String, String)>>,
    models: Option<Vec<String>>,
) -> PyResult<String> {
    let agent = extract_orchestrator(py, &orch)?;

    let mut auth = StaticTokens::new();
    if let Some(map) = tokens {
        for (token, (tenant, user)) in map {
            auth = auth.with(token, Principal::new(tenant, user));
        }
    } else {
        // Default: a single dev token so smoke tests work without
        // credential setup. Production callers should always pass
        // their own map.
        auth = auth.with("dev-token", Principal::new("anonymous", "anonymous"));
    }

    let config = ServeConfig {
        host: host.to_string(),
        port,
        models: models.unwrap_or_else(|| vec!["tako-default".into()]),
    };

    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let (addr, handle) = py
        .detach(|| rt.block_on(serve_openai(agent, Arc::new(auth), config)))
        .map_err(map_err)?;

    let mut slot = server_slot()
        .lock()
        .map_err(|e| PyValueError::new_err(format!("server slot poisoned: {e}")))?;
    if let Some(prev) = slot.take() {
        prev.abort();
    }
    *slot = Some(handle);

    Ok(format!("http://{addr}"))
}

#[pyfunction]
pub fn shutdown_compat_py() -> PyResult<()> {
    let mut slot = server_slot()
        .lock()
        .map_err(|e| PyValueError::new_err(format!("server slot poisoned: {e}")))?;
    if let Some(handle) = slot.take() {
        handle.abort();
    }
    Ok(())
}

fn extract_orchestrator(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn Orchestrator>> {
    let bound = obj.bind(py);
    if let Ok(o) = bound.cast::<crate::py_orchestrator::PyOrchestrator>() {
        return Ok(o.borrow().inner_arc());
    }
    if let Ok(o) = bound.cast::<crate::py_conductor::PyConductor>() {
        return Ok(o.borrow().inner_arc());
    }
    Err(PyValueError::new_err(
        "expected a tako._native.Orchestrator or Conductor",
    ))
}
