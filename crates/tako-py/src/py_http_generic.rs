//! `PyHttpGeneric` — wraps `tako-providers-http-generic::HttpGenericProvider`.
//!
//! `body_template` and `stream_config` are accepted as Python values
//! (dict / list / scalar / None) and converted to `serde_json::Value`
//! via [`crate::conv::py_to_json`]. The serde derives on
//! [`HttpGenericConfig`] and the `#[serde(tag = "kind", rename_all =
//! "snake_case")]` representation of [`StreamConfig`] mean Python passes
//! e.g. `{"kind": "openai_sse", "content_pointer": "..."}` and serde
//! handles the dispatch — no enum-mapping plumbing needed in PyO3.
//!
//! Construction is synchronous (the upstream builder doesn't `.await`),
//! so unlike `PyBedrock` we don't need `block_on` or GIL detach.

use std::sync::Arc;

use pyo3::prelude::*;
use serde_json::Value;
use tako_providers_http_generic::{HttpGenericConfig, HttpGenericProvider, StreamConfig};

use crate::conv::py_to_json;
use crate::py_provider::{ProviderHandle, map_err};

#[pyclass(name = "HttpGeneric", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyHttpGeneric {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyHttpGeneric {
    /// Build an `HttpGenericProvider`.
    ///
    /// `body_template` is a JSON-shaped Python value (dict / list /
    /// scalar) where the literal strings `"{{ request }}"`,
    /// `"{{ model }}"`, `"{{ messages }}"` are substituted at call
    /// time. `response_text_pointer` is a JSON Pointer (RFC 6901) into
    /// the response body that yields the assistant text.
    ///
    /// Pass `stream_config={"kind": "openai_sse", ...}` or
    /// `{"kind": "ndjson", ...}` to enable streaming;
    /// `Capabilities::supports_streaming` flips automatically.
    /// `headers` may carry `"$VAR_NAME"` literals; the provider
    /// resolves those from the environment at construction.
    #[new]
    #[pyo3(signature = (
        id,
        model,
        url,
        body_template,
        response_text_pointer,
        *,
        headers=None,
        timeout_secs=None,
        stream_config=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: String,
        model: String,
        url: String,
        body_template: &Bound<'_, PyAny>,
        response_text_pointer: String,
        headers: Option<Vec<(String, String)>>,
        timeout_secs: Option<u64>,
        stream_config: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let body_template: Value = py_to_json(body_template)?;
        let stream_config: Option<StreamConfig> = match stream_config {
            None => None,
            Some(obj) if obj.is_none() => None,
            Some(obj) => {
                let v = py_to_json(obj)?;
                Some(serde_json::from_value(v).map_err(|e| {
                    pyo3::exceptions::PyValueError::new_err(format!("invalid stream_config: {e}"))
                })?)
            }
        };

        let cfg = HttpGenericConfig {
            id,
            model,
            url,
            headers: headers.unwrap_or_default(),
            body_template,
            response_text_pointer,
            capabilities: None,
            timeout_secs,
            stream_config,
        };
        let provider = HttpGenericProvider::new(cfg).map_err(map_err)?;
        Ok(Self {
            handle: ProviderHandle {
                inner: Arc::new(provider),
            },
        })
    }

    fn id(&self) -> &str {
        self.handle.inner.id()
    }

    /// Returns `true` if a `stream_config` was supplied (or an explicit
    /// `Capabilities` override turned the flag on). Useful for round-
    /// tripping the streaming-capability bit from Python.
    fn supports_streaming(&self) -> bool {
        self.handle.inner.capabilities().supports_streaming
    }
}
