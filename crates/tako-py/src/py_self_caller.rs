//! `PySelfCaller` — wraps `tako_orchestrator::SelfCaller`.
//!
//! Also exposes the two built-in `ConfidenceGuard` impls
//! ([`PyRuleBasedGuard`], [`PyLlmJudgeGuard`]) so users can build a
//! SelfCaller from pure Python without writing a custom guard.

use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;
use tako_core::ConfidenceGuard;
use tako_orchestrator::{LlmJudgeGuard, OrchInput, Orchestrator, RuleBasedGuard, SelfCaller};
use tako_runtime::BudgetTracker;

use crate::py_conductor::{PyConductor, extract_any_provider};
use crate::py_governance::PyBudget;
use crate::py_orch_event::PyOrchEventStream;
use crate::py_orchestrator::PyOrchestrator;
use crate::py_trinity::PyTrinity;

#[pyclass(name = "RuleBasedGuard", module = "tako._native", skip_from_py_object)]
pub struct PyRuleBasedGuard {
    pub(crate) inner: Arc<dyn ConfidenceGuard>,
}

impl std::fmt::Debug for PyRuleBasedGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyRuleBasedGuard").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyRuleBasedGuard {
    /// Build a rule guard that requires the output be at least
    /// `min_chars` long. If `pattern` is given, also requires the regex
    /// to match.
    #[new]
    #[pyo3(signature = (min_chars=0, pattern=None))]
    fn new(min_chars: usize, pattern: Option<&str>) -> PyResult<Self> {
        let mut g = RuleBasedGuard::new(min_chars);
        if let Some(p) = pattern {
            let re = regex::Regex::new(p).map_err(|e| PyValueError::new_err(e.to_string()))?;
            g = g.with_pattern(re);
        }
        Ok(Self {
            inner: Arc::new(g) as Arc<dyn ConfidenceGuard>,
        })
    }
}

#[pyclass(name = "LlmJudgeGuard", module = "tako._native", skip_from_py_object)]
pub struct PyLlmJudgeGuard {
    pub(crate) inner: Arc<dyn ConfidenceGuard>,
}

impl std::fmt::Debug for PyLlmJudgeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyLlmJudgeGuard").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyLlmJudgeGuard {
    /// Build an LLM-judge guard. `judge` is any tako provider (OpenAI,
    /// Anthropic, ...). `rubric` is the system-style prompt the judge
    /// scores against. Optionally accepts `budget=` and
    /// `budget_backend=` kwargs to meter the judge's own provider call
    /// — distinct from the inner orchestrator's budget which covers
    /// regular execution.
    ///
    /// Phase 9.A streaming opt-in: pass `streaming_min_chars=` to
    /// enable per-N-delta judging from `evaluate_streaming` (default
    /// `None`, i.e. streaming evaluation disabled — preserves the
    /// v0.9.0 behaviour). Pass `streaming_every_n=` to throttle the
    /// judge call to every Nth over-threshold partial (default `1`).
    #[new]
    #[pyo3(signature = (
        judge, rubric,
        budget=None, budget_backend=None,
        streaming_min_chars=None, streaming_every_n=None,
    ))]
    fn new(
        py: Python<'_>,
        judge: Py<PyAny>,
        rubric: String,
        budget: Option<PyBudget>,
        budget_backend: Option<Py<PyAny>>,
        streaming_min_chars: Option<usize>,
        streaming_every_n: Option<u32>,
    ) -> PyResult<Self> {
        let provider = extract_any_provider(py, &judge)?;
        let mut guard = LlmJudgeGuard::new(provider, rubric);
        if budget.is_some() || budget_backend.is_some() {
            let budget_inner = budget.map(|b| b.inner).unwrap_or_default();
            let backend = if let Some(obj) = budget_backend {
                crate::py_runtime::extract_budget_backend(py, &obj)?
            } else {
                Arc::new(tako_runtime::InMemoryBudgetBackend::new())
                    as Arc<dyn tako_runtime::BudgetBackend>
            };
            let tracker = Arc::new(BudgetTracker::new(backend, budget_inner));
            guard = guard.with_budget(tracker);
        }
        if let Some(n) = streaming_min_chars {
            guard = guard.with_streaming_min_chars(n);
        }
        if let Some(n) = streaming_every_n {
            guard = guard.with_streaming_every_n(n);
        }
        Ok(Self {
            inner: Arc::new(guard) as Arc<dyn ConfidenceGuard>,
        })
    }
}

#[pyclass(name = "SelfCaller", module = "tako._native", skip_from_py_object)]
pub struct PySelfCaller {
    pub(crate) inner: Arc<SelfCaller>,
}

impl PySelfCaller {
    #[allow(dead_code)]
    pub(crate) fn inner_arc(&self) -> Arc<dyn Orchestrator> {
        Arc::clone(&self.inner) as Arc<dyn Orchestrator>
    }
}

impl std::fmt::Debug for PySelfCaller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PySelfCaller").finish_non_exhaustive()
    }
}

#[pymethods]
impl PySelfCaller {
    /// Build a SelfCaller wrapping any tako orchestrator. `inner` may be
    /// `tako._native.{Orchestrator, Conductor, Trinity}`. `confidence`
    /// must be a `RuleBasedGuard` or `LlmJudgeGuard`.
    #[new]
    #[pyo3(signature = (inner, confidence, max_depth=3, min_confidence=0.7, revision_prompt=None))]
    fn new(
        py: Python<'_>,
        inner: Py<PyAny>,
        confidence: Py<PyAny>,
        max_depth: u8,
        min_confidence: f32,
        revision_prompt: Option<String>,
    ) -> PyResult<Self> {
        let inner_arc = extract_any_orchestrator(py, &inner)?;
        let conf_arc = extract_any_guard(py, &confidence)?;
        let mut builder = SelfCaller::builder()
            .inner(inner_arc)
            .confidence(conf_arc)
            .max_depth(max_depth)
            .min_confidence(min_confidence);
        if let Some(p) = revision_prompt {
            builder = builder.revision_prompt(p);
        }
        let sc = builder
            .build()
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(sc),
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

    /// Async-iterable stream of [`PyOrchEvent`]s
    /// (kinds: `step_start`, `assistant_text`, `tool_call_start`,
    /// `tool_call_result`, `final`).
    ///
    /// Inner `final` events are absorbed by the recursion loop; the
    /// stream yields exactly one outer `final` event when the
    /// confidence guard accepts an output (or `max_depth` is hit).
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

fn extract_any_orchestrator(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn Orchestrator>> {
    if let Ok(o) = obj.extract::<PyRef<PyOrchestrator>>(py) {
        return Ok(o.inner_arc());
    }
    if let Ok(o) = obj.extract::<PyRef<PyConductor>>(py) {
        return Ok(o.inner_arc());
    }
    if let Ok(o) = obj.extract::<PyRef<PyTrinity>>(py) {
        return Ok(o.inner_arc());
    }
    Err(PyValueError::new_err(
        "inner must be tako._native.Orchestrator, Conductor, or Trinity",
    ))
}

fn extract_any_guard(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn ConfidenceGuard>> {
    if let Ok(g) = obj.extract::<PyRef<PyRuleBasedGuard>>(py) {
        return Ok(Arc::clone(&g.inner));
    }
    if let Ok(g) = obj.extract::<PyRef<PyLlmJudgeGuard>>(py) {
        return Ok(Arc::clone(&g.inner));
    }
    Err(PyValueError::new_err(
        "confidence must be tako._native.RuleBasedGuard or LlmJudgeGuard",
    ))
}
