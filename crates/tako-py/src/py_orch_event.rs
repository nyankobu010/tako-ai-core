//! Python-visible types for orchestrator streaming.
//!
//! Phase 7.B introduces the first streaming entry point on the Python
//! facade — `tako.SelfCaller.stream(...)` — and lands these shared
//! types so future Trinity / SingleAgent stream bindings can reuse the
//! same shape without redesigning the wire format.
//!
//! - [`PyOrchEvent`] is a read-only wrapper around a single
//!   [`tako_orchestrator::OrchEvent`]. The `kind` getter returns one of
//!   `"step_start" | "assistant_text" | "tool_call_start" |
//!   "tool_call_result" | "final"`; per-variant getters expose the
//!   payload fields (returning `None` when the field doesn't apply).
//! - [`PyOrchEventStream`] is an async iterator (`__aiter__` + async
//!   `__anext__`) over a `BoxStream` of events. Constructing one
//!   parks the stream behind a `tokio::sync::Mutex` so the pyclass
//!   stays `Send + Sync`; each `__anext__` call locks, polls the next
//!   event, and releases.

use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;
use pyo3::exceptions::{PyStopAsyncIteration, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3_async_runtimes::tokio::future_into_py;
use tako_core::TakoError;
use tako_orchestrator::OrchEvent;
use tokio::sync::Mutex as TokioMutex;

type EventStream = BoxStream<'static, Result<OrchEvent, TakoError>>;

/// Python-visible single orchestrator event.
#[pyclass(name = "OrchEvent", module = "tako._native", frozen)]
#[derive(Debug)]
pub struct PyOrchEvent {
    inner: OrchEvent,
}

impl PyOrchEvent {
    pub(crate) fn from_event(ev: OrchEvent) -> Self {
        Self { inner: ev }
    }
}

#[pymethods]
impl PyOrchEvent {
    /// Discriminant: one of `"step_start" | "assistant_text" |
    /// "tool_call_start" | "tool_call_result" | "final"`.
    #[getter]
    fn kind(&self) -> &'static str {
        match &self.inner {
            OrchEvent::StepStart { .. } => "step_start",
            OrchEvent::AssistantText { .. } => "assistant_text",
            OrchEvent::ToolCallStart { .. } => "tool_call_start",
            OrchEvent::ToolCallResult { .. } => "tool_call_result",
            OrchEvent::Final { .. } => "final",
        }
    }

    /// Step index for `step_start`, `assistant_text`,
    /// `tool_call_start`, `tool_call_result`. `None` for `final`.
    #[getter]
    fn step(&self) -> Option<u32> {
        match &self.inner {
            OrchEvent::StepStart { step }
            | OrchEvent::AssistantText { step, .. }
            | OrchEvent::ToolCallStart { step, .. }
            | OrchEvent::ToolCallResult { step, .. } => Some(*step),
            OrchEvent::Final { .. } => None,
        }
    }

    /// Text delta for `assistant_text`; `None` for other variants.
    #[getter]
    fn delta(&self) -> Option<&str> {
        match &self.inner {
            OrchEvent::AssistantText { delta, .. } => Some(delta),
            _ => None,
        }
    }

    /// Tool name for `tool_call_start`; `None` otherwise.
    #[getter]
    fn name(&self) -> Option<&str> {
        match &self.inner {
            OrchEvent::ToolCallStart { name, .. } => Some(name),
            _ => None,
        }
    }

    /// Tool-call id for `tool_call_start` / `tool_call_result`; `None`
    /// otherwise.
    #[getter]
    fn id(&self) -> Option<&str> {
        match &self.inner {
            OrchEvent::ToolCallStart { id, .. } | OrchEvent::ToolCallResult { id, .. } => Some(id),
            _ => None,
        }
    }

    /// Tool result JSON for `tool_call_result`; `None` otherwise.
    #[getter]
    fn result<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        match &self.inner {
            OrchEvent::ToolCallResult { result, .. } => {
                let s = serde_json::to_string(result)
                    .map_err(|e| PyValueError::new_err(e.to_string()))?;
                let json = py.import("json")?;
                let loaded = json.call_method1("loads", (s,))?;
                Ok(Some(loaded))
            }
            _ => Ok(None),
        }
    }

    /// `is_error` flag for `tool_call_result`; `None` otherwise.
    #[getter]
    fn is_error(&self) -> Option<bool> {
        match &self.inner {
            OrchEvent::ToolCallResult { is_error, .. } => Some(*is_error),
            _ => None,
        }
    }

    /// Final text for `final`; `None` otherwise.
    #[getter]
    fn text(&self) -> Option<&str> {
        match &self.inner {
            OrchEvent::Final { output } => Some(&output.text),
            _ => None,
        }
    }

    /// Final usage `{input_tokens, output_tokens}` dict for `final`;
    /// `None` otherwise.
    #[getter]
    fn usage<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        match &self.inner {
            OrchEvent::Final { output } => {
                let d = PyDict::new(py);
                d.set_item("input_tokens", output.usage.input_tokens)?;
                d.set_item("output_tokens", output.usage.output_tokens)?;
                Ok(Some(d))
            }
            _ => Ok(None),
        }
    }

    fn __repr__(&self) -> String {
        format!("OrchEvent(kind={})", self.kind())
    }
}

/// Async-iterable stream of [`PyOrchEvent`]s.
#[pyclass(name = "OrchEventStream", module = "tako._native")]
pub struct PyOrchEventStream {
    stream: Arc<TokioMutex<EventStream>>,
}

impl std::fmt::Debug for PyOrchEventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyOrchEventStream").finish_non_exhaustive()
    }
}

impl PyOrchEventStream {
    pub(crate) fn new(stream: EventStream) -> Self {
        Self {
            stream: Arc::new(TokioMutex::new(stream)),
        }
    }
}

#[pymethods]
impl PyOrchEventStream {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let s = Arc::clone(&self.stream);
        future_into_py(py, async move {
            let mut guard = s.lock().await;
            match guard.next().await {
                Some(Ok(ev)) => Ok(PyOrchEvent::from_event(ev)),
                Some(Err(e)) => Err(PyValueError::new_err(e.to_string())),
                None => Err(PyStopAsyncIteration::new_err(())),
            }
        })
    }
}
