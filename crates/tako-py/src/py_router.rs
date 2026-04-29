//! `PyRegexRouter` and (feature-gated) `PyOnnxRouter` — Python wrappers
//! over the `Router` impls in `tako-orchestrator`.

use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_core::Router;
use tako_orchestrator::RegexRouter;

#[pyclass(name = "RegexRouter", module = "tako._native", skip_from_py_object)]
pub struct PyRegexRouter {
    pub(crate) inner: Arc<dyn Router>,
}

impl std::fmt::Debug for PyRegexRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyRegexRouter").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyRegexRouter {
    /// Build the default rule-based router (code/math/fallback split).
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(RegexRouter::default()) as Arc<dyn Router>,
        }
    }
}

#[cfg(feature = "onnx")]
#[pyclass(name = "OnnxRouter", module = "tako._native", skip_from_py_object)]
pub struct PyOnnxRouter {
    pub(crate) inner: Arc<dyn Router>,
}

#[cfg(feature = "onnx")]
impl std::fmt::Debug for PyOnnxRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyOnnxRouter").finish_non_exhaustive()
    }
}

#[cfg(feature = "onnx")]
#[pymethods]
impl PyOnnxRouter {
    /// Load a classifier from an ONNX file. The classifier must take a
    /// `float32[1, FEATURE_DIM]` input named `features` and emit a
    /// `float32[1, K]` output named `logits`.
    #[new]
    fn new(path: String) -> Self {
        Self {
            inner: Arc::new(tako_orchestrator::OnnxRouter::from_path(path)) as Arc<dyn Router>,
        }
    }
}

/// Helper: extract a `Router` Arc from a Python object that may be either
/// `PyRegexRouter` or (feature-gated) `PyOnnxRouter`.
#[allow(dead_code)]
pub(crate) fn extract_router(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn Router>> {
    if let Ok(r) = obj.extract::<PyRef<PyRegexRouter>>(py) {
        return Ok(Arc::clone(&r.inner));
    }
    #[cfg(feature = "onnx")]
    {
        if let Ok(r) = obj.extract::<PyRef<PyOnnxRouter>>(py) {
            return Ok(Arc::clone(&r.inner));
        }
    }
    Err(PyValueError::new_err(
        "router must be tako._native.RegexRouter (or OnnxRouter when built with --features onnx)",
    ))
}
