//! `tako-py` — PyO3 bindings, exposed as the `tako._native` extension module.
//!
//! GIL discipline (per spec §12, enforced in code review):
//!
//! 1. The shared Tokio runtime comes from
//!    [`pyo3_async_runtimes::tokio::get_runtime`]; we never build a second.
//! 2. Async wrappers convert futures via
//!    [`pyo3_async_runtimes::tokio::future_into_py`].
//! 3. Sync siblings (`*_sync`) call `py.detach(|| rt.block_on(...))` so
//!    other Python threads can take the GIL while we wait.
//! 4. Inside futures we never call `Python::attach` while holding I/O.

#![allow(unsafe_code)] // PyO3 macro expansions emit unsafe extern blocks; required for FFI.

mod conv;
mod py_bedrock;
mod py_compat;
mod py_conductor;
mod py_governance;
mod py_mcp;
mod py_orchestrator;
mod py_provider;
mod py_python_provider;

use pyo3::prelude::*;

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<py_provider::PyOpenAI>()?;
    m.add_class::<py_provider::PyAnthropic>()?;
    m.add_class::<py_provider::PyFakeProvider>()?;
    m.add_class::<py_bedrock::PyBedrock>()?;
    m.add_class::<py_python_provider::PyPythonProvider>()?;
    m.add_class::<py_orchestrator::PyOrchestrator>()?;
    m.add_class::<py_conductor::PyConductor>()?;
    m.add_class::<py_mcp::PyStdio>()?;
    m.add_class::<py_mcp::PyStreamableHttp>()?;
    m.add_class::<py_governance::PyBudget>()?;
    m.add_function(wrap_pyfunction!(py_governance::init_tracing_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_governance::init_otlp_tracing_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_governance::shutdown_otlp_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_compat::serve_openai_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_compat::shutdown_compat_py, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
