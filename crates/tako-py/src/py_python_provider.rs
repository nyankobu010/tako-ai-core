//! `PyPythonProvider` — a `LlmProvider` whose `chat()` is a Python
//! async callable.
//!
//! GIL hand-off:
//! 1. We attach to Python long enough to call the user's `chat(request)`
//!    method, which returns a coroutine.
//! 2. `pyo3_async_runtimes::tokio::into_future` converts that coroutine
//!    to a Rust future without holding the GIL.
//! 3. We `.await` outside of `Python::attach`.
//! 4. We re-attach to extract the result string.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use serde_json::Value;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, ContentPart, FinishReason, LlmProvider,
    Message, Principal, Role, TakoError, Usage,
};

use crate::py_provider::ProviderHandle;

/// The Rust side of a Python-defined provider. Holds the user's async
/// callable as a `Py<PyAny>` and forwards `chat` calls.
struct PyImpl {
    id: String,
    capabilities: Capabilities,
    /// `async def chat(request: dict) -> str`
    chat_callable: Py<PyAny>,
}

impl std::fmt::Debug for PyImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyImpl")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl LlmProvider for PyImpl {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn chat(
        &self,
        _principal: &Principal,
        req: ChatRequest,
    ) -> Result<ChatResponse, TakoError> {
        // Step 1: serialise the request to a Python dict and call the
        // user's `chat` callable. This is the only stretch that needs
        // the GIL; the awaitable conversion happens here too.
        let coro_future = Python::attach(|py| -> PyResult<_> {
            let req_value =
                serde_json::to_value(&req).map_err(|e| PyValueError::new_err(e.to_string()))?;
            let req_py = json_value_to_py(py, &req_value)?;
            let result = self.chat_callable.call1(py, (req_py,))?;
            let coro = result.into_bound(py);
            // into_future converts the Python awaitable into a Rust future
            // and releases the GIL while it's pending.
            pyo3_async_runtimes::tokio::into_future(coro)
        })
        .map_err(|e| TakoError::Provider {
            message: format!("Python provider raised on dispatch: {e}"),
            source: None,
            details: Box::new(tako_core::ProviderErrorDetails {
                provider_id: self.id.clone(),
                model: req.model.clone(),
                ..Default::default()
            }),
        })?;

        // Step 2: await the Python coroutine outside `Python::attach`.
        let py_result = coro_future.await.map_err(|e| TakoError::Provider {
            message: format!("Python provider raised: {e}"),
            source: None,
            details: Box::new(tako_core::ProviderErrorDetails {
                provider_id: self.id.clone(),
                model: req.model.clone(),
                ..Default::default()
            }),
        })?;

        // Step 3: extract the result text. We accept either a plain string
        // or a {"text": "...", "input_tokens": int, "output_tokens": int} dict.
        let (text, usage) = Python::attach(|py| -> PyResult<(String, Usage)> {
            let bound = py_result.into_bound(py);
            if let Ok(s) = bound.extract::<String>() {
                return Ok((s, Usage::default()));
            }
            if let Ok(dict) = bound.cast::<PyDict>() {
                let text: String = dict
                    .get_item("text")?
                    .ok_or_else(|| PyValueError::new_err("dict response missing 'text'"))?
                    .extract()?;
                let input_tokens: u32 = dict
                    .get_item("input_tokens")?
                    .map(|v| v.extract::<u32>())
                    .transpose()?
                    .unwrap_or(0);
                let output_tokens: u32 = dict
                    .get_item("output_tokens")?
                    .map(|v| v.extract::<u32>())
                    .transpose()?
                    .unwrap_or(0);
                return Ok((
                    text,
                    Usage {
                        input_tokens,
                        output_tokens,
                    },
                ));
            }
            Err(PyTypeError::new_err(
                "Python provider chat() must return str or {'text': str, 'input_tokens'?: int, 'output_tokens'?: int}",
            ))
        })
        .map_err(|e| TakoError::Provider {
            message: format!("Python provider returned bad type: {e}"),
            source: None,
            details: Box::new(tako_core::ProviderErrorDetails {
                provider_id: self.id.clone(),
                model: req.model.clone(),
                ..Default::default()
            }),
        })?;

        Ok(ChatResponse {
            message: Message {
                role: Role::Assistant,
                content: vec![ContentPart::Text { text }],
            },
            finish_reason: FinishReason::Stop,
            usage,
            raw: Default::default(),
        })
    }

    async fn stream(
        &self,
        _p: &Principal,
        _r: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        Err(TakoError::Invalid(
            "Python providers do not yet support streaming (Phase 2)".into(),
        ))
    }
}

#[pyclass(name = "PythonProvider", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyPythonProvider {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyPythonProvider {
    /// Build a Rust-side `LlmProvider` that dispatches to a Python async
    /// callable.
    ///
    /// `chat` must be `async def chat(request: dict) -> str | dict`. The
    /// dict form lets the callable report token usage:
    /// `{"text": "...", "input_tokens": 5, "output_tokens": 3}`.
    #[new]
    #[pyo3(signature = (id, chat, max_context_tokens=None))]
    fn new(id: String, chat: Py<PyAny>, max_context_tokens: Option<u32>) -> PyResult<Self> {
        let capabilities = Capabilities {
            max_context_tokens: max_context_tokens.unwrap_or(32_000),
            supports_streaming: false,
            supports_tools: false,
            supports_vision: false,
            supports_json_mode: false,
            usd_per_input_mtok: None,
            usd_per_output_mtok: None,
        };
        let inner = PyImpl {
            id,
            capabilities,
            chat_callable: chat,
        };
        Ok(Self {
            handle: ProviderHandle {
                inner: Arc::new(inner),
            },
        })
    }

    fn id(&self) -> &str {
        self.handle.inner.id()
    }
}

/// Local copy of the json→py converter (also lives in conv.rs but is
/// behind dead-code allow there).
fn json_value_to_py<'py>(py: Python<'py>, v: &Value) -> PyResult<Bound<'py, PyAny>> {
    use pyo3::types::PyList;
    match v {
        Value::Null => Ok(py.None().into_bound(py)),
        Value::Bool(b) => Ok(b.into_pyobject(py)?.to_owned().into_any()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_pyobject(py)?.to_owned().into_any())
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_pyobject(py)?.to_owned().into_any())
            } else {
                Ok(py.None().into_bound(py))
            }
        }
        Value::String(s) => Ok(s.into_pyobject(py)?.to_owned().into_any()),
        Value::Array(arr) => {
            let list = PyList::empty(py);
            for item in arr {
                list.append(json_value_to_py(py, item)?)?;
            }
            Ok(list.into_any())
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, val) in map {
                dict.set_item(k, json_value_to_py(py, val)?)?;
            }
            Ok(dict.into_any())
        }
    }
}
