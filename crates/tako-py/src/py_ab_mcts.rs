//! `PyAbMcts` — wraps `tako_orchestrator::AbMcts` (Phase 8.B).
//!
//! Closes the v0.5.0 gap: AB-MCTS landed in Rust but had no Python
//! facade. This module exposes both the orchestrator and a built-in
//! [`PyRuleBasedVerifier`] so callers can construct AB-MCTS from pure
//! Python without writing a custom verifier.

use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;
use tako_core::Verifier;
use tako_orchestrator::{AbMcts, OrchInput, Orchestrator, RuleBasedVerifier};

use crate::py_conductor::extract_any_provider;
use crate::py_orch_event::PyOrchEventStream;

#[pyclass(
    name = "RuleBasedVerifier",
    module = "tako._native",
    skip_from_py_object
)]
pub struct PyRuleBasedVerifier {
    pub(crate) inner: Arc<dyn Verifier>,
}

impl std::fmt::Debug for PyRuleBasedVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyRuleBasedVerifier")
            .finish_non_exhaustive()
    }
}

#[pymethods]
impl PyRuleBasedVerifier {
    /// Rule-based verifier that returns 1.0 when the rollout text is at
    /// least `min_chars` long and (optionally) matches `pattern`. Below
    /// `min_chars`, returns a partial proportional score.
    #[new]
    #[pyo3(signature = (min_chars=0, pattern=None))]
    fn new(min_chars: usize, pattern: Option<&str>) -> PyResult<Self> {
        let mut v = RuleBasedVerifier::new(min_chars);
        if let Some(p) = pattern {
            let re = regex::Regex::new(p).map_err(|e| PyValueError::new_err(e.to_string()))?;
            v = v.with_pattern(re);
        }
        Ok(Self {
            inner: Arc::new(v) as Arc<dyn Verifier>,
        })
    }
}

#[pyclass(name = "AbMcts", module = "tako._native", skip_from_py_object)]
pub struct PyAbMcts {
    pub(crate) inner: Arc<AbMcts>,
}

impl PyAbMcts {
    #[allow(dead_code)]
    pub(crate) fn inner_arc(&self) -> Arc<dyn Orchestrator> {
        Arc::clone(&self.inner) as Arc<dyn Orchestrator>
    }
}

impl std::fmt::Debug for PyAbMcts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyAbMcts").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyAbMcts {
    /// Build an AB-MCTS orchestrator.
    ///
    /// `provider` is any tako provider; `verifier` must be a
    /// `tako._native.RuleBasedVerifier` (further verifier types can be
    /// added in follow-on releases).
    ///
    /// Phase 9.D: pass `candidates=[p1, p2, ...]` to register
    /// additional providers and `router=tako._native.RegexRouter()` (or
    /// `OnnxRouter`) to enable router-driven branch expansion. The
    /// router runs once per rollout (per branch expansion) over
    /// `[primary, ...candidates]`. Without `router`, candidates are
    /// ignored and every rollout uses the primary provider.
    #[new]
    #[pyo3(signature = (
        provider,
        verifier,
        max_iterations=16,
        branching_factor=3,
        max_steps_per_rollout=4,
        temperature=0.7,
        min_confidence=0.95,
        candidates=None,
        router=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        provider: Py<PyAny>,
        verifier: Py<PyAny>,
        max_iterations: u32,
        branching_factor: u32,
        max_steps_per_rollout: u32,
        temperature: f32,
        min_confidence: f32,
        candidates: Option<Vec<Py<PyAny>>>,
        router: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let provider_arc = extract_any_provider(py, &provider)?;
        let verifier_arc = extract_any_verifier(py, &verifier)?;
        let mut builder = AbMcts::builder()
            .provider(provider_arc)
            .verifier(verifier_arc)
            .max_iterations(max_iterations)
            .branching_factor(branching_factor)
            .max_steps_per_rollout(max_steps_per_rollout)
            .temperature(temperature)
            .min_confidence(min_confidence);
        if let Some(cands) = candidates {
            for c in cands {
                let cand_arc = extract_any_provider(py, &c)?;
                builder = builder.candidate(cand_arc);
            }
        }
        if let Some(r) = router {
            let router_arc = crate::py_router::extract_router(py, &r)?;
            builder = builder.router(router_arc);
        }
        let mcts = builder
            .build()
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(mcts),
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

    /// Async-iterable stream of [`crate::py_orch_event::PyOrchEvent`]s.
    /// Per iteration: `step_start` → `assistant_text` (full rollout
    /// text) → `verifier_score` (with `branch` and `score`). After all
    /// iterations or `min_confidence` early-stop, exactly one terminal
    /// `final` event closes the stream.
    #[pyo3(signature = (prompt, tenant_id=None, user_id=None))]
    fn stream<'py>(
        &self,
        py: Python<'py>,
        prompt: String,
        tenant_id: Option<String>,
        user_id: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let principal = crate::conv::principal_from(tenant_id.as_deref(), user_id.as_deref());
        future_into_py(py, async move {
            let s = inner.stream(&principal, OrchInput::from_user(prompt)).await;
            Ok(PyOrchEventStream::new(s))
        })
    }
}

pub(crate) fn extract_any_verifier(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn Verifier>> {
    if let Ok(v) = obj.extract::<PyRef<PyRuleBasedVerifier>>(py) {
        return Ok(Arc::clone(&v.inner));
    }
    Err(PyValueError::new_err(
        "verifier must be tako._native.RuleBasedVerifier",
    ))
}
