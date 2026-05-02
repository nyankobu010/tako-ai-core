//! Phase 46.B — Python-visible orchestrator output.
//!
//! Returned by `Orchestrator.run` / `run_sync` (and the matching
//! methods on `Conductor`, `SelfCaller`, `AbMcts`, `Trinity`).
//! Replaces the Phase-1 placeholder where these methods returned
//! a bare `String`. The Rust [`tako_orchestrator::OrchOutput`]
//! type carries `text`, `message`, `usage`, and `steps`; this
//! pyclass exposes everything except `message` (which would need
//! `ContentPart`/`Message` round-tripping machinery to expose
//! cleanly — deferred until operator ask).
//!
//! `text` is the only field whose name was promised stable in
//! the original placeholder docstring. `input_tokens`,
//! `output_tokens`, `total_tokens`, and `steps` are additive.

use pyo3::prelude::*;
use tako_orchestrator::OrchOutput;

#[pyclass(name = "OrchOutput", module = "tako._native", frozen)]
#[derive(Debug)]
pub struct PyOrchOutput {
    inner: OrchOutput,
}

impl PyOrchOutput {
    pub(crate) fn new(inner: OrchOutput) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyOrchOutput {
    /// Concatenated assistant text from the final turn. The only
    /// field whose name is guaranteed stable from Phase 1.
    #[getter]
    fn text(&self) -> &str {
        &self.inner.text
    }

    /// Phase 46.B — input tokens (prompt) summed across all
    /// provider calls in the run.
    #[getter]
    fn input_tokens(&self) -> u32 {
        self.inner.usage.input_tokens
    }

    /// Phase 46.B — output tokens (completion) summed across
    /// all provider calls in the run.
    #[getter]
    fn output_tokens(&self) -> u32 {
        self.inner.usage.output_tokens
    }

    /// Phase 46.B — sum of input + output tokens. Mirrors
    /// [`tako_core::Usage::total`].
    #[getter]
    fn total_tokens(&self) -> u32 {
        self.inner.usage.total()
    }

    /// Phase 46.B — number of provider calls (assistant turns
    /// produced) in the run. `1` for a single-shot agent that
    /// answered without invoking any tool.
    #[getter]
    fn steps(&self) -> u32 {
        self.inner.steps
    }

    fn __repr__(&self) -> String {
        // Truncate text snippet at 60 chars for repr — same convention as
        // the prior Python `_Result.__repr__`.
        let snippet = if self.inner.text.chars().count() > 60 {
            format!(
                "{}...",
                self.inner.text.chars().take(60).collect::<String>()
            )
        } else {
            self.inner.text.clone()
        };
        format!(
            "OrchOutput(text={snippet:?}, input_tokens={}, output_tokens={}, steps={})",
            self.inner.usage.input_tokens, self.inner.usage.output_tokens, self.inner.steps,
        )
    }
}
