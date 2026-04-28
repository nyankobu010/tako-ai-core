//! `tako-py` — PyO3 bindings for tako, exposed as the `tako._native` extension module.

use pyo3::prelude::*;

#[pymodule]
fn _native(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    // PyClient, PyOrchestrator, providers, MCP, OTel, Budget bindings
    // are registered in subsequent commits.
    Ok(())
}
