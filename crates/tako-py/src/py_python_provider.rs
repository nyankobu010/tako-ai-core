//! `PyPythonProvider` — a `LlmProvider` whose `chat()` (and optional
//! `stream()`) is a Python async callable.
//!
//! GIL hand-off:
//! 1. We attach to Python long enough to call the user's `chat(request)`
//!    method, which returns a coroutine.
//! 2. `pyo3_async_runtimes::tokio::into_future` converts that coroutine
//!    to a Rust future without holding the GIL.
//! 3. We `.await` outside of `Python::attach`.
//! 4. We re-attach to extract the result string.
//!
//! Streaming (Phase 10.D) follows the same hand-off applied per chunk:
//! we attach to Python to call `__anext__()` on the async iterator,
//! release the GIL while awaiting the resulting coroutine, then
//! re-attach to deserialise the yielded dict into a `ChatChunk` via
//! the standard `serde(tag = "kind", rename_all = "snake_case")`
//! representation. `StopAsyncIteration` cleanly terminates the stream.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use pyo3::exceptions::{PyStopAsyncIteration, PyTypeError, PyValueError};
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
    /// `async def chat(request: dict) -> str | dict`
    chat_callable: Py<PyAny>,
    /// Phase 10.D — optional streaming callable. When set, the
    /// provider's `Capabilities::supports_streaming` is `true` and
    /// `stream()` invokes this callable. Contract: `async def
    /// stream(request: dict) -> AsyncIterator[dict]` whose yielded
    /// dicts deserialise as [`ChatChunk`] via its `kind`-tagged
    /// JSON shape (e.g. `{"kind": "delta", "text": "..."}` or
    /// `{"kind": "end", "finish_reason": "stop", "usage": {
    /// "input_tokens": 5, "output_tokens": 3}}`).
    stream_callable: Option<Py<PyAny>>,
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
        _principal: &Principal,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        if self.stream_callable.is_none() {
            return Err(TakoError::Invalid(format!(
                "python provider `{}` did not register a stream callable; pass `stream=...` to PythonProvider(...)",
                self.id,
            )));
        }
        let stream_callable: Py<PyAny> = Python::attach(|py| {
            self.stream_callable
                .as_ref()
                .map(|s| s.clone_ref(py))
                .ok_or_else(|| TakoError::Invalid("stream callable disappeared".into()))
        })?;

        // Step 1 (GIL): build the request dict and call the user's
        // `stream(request)`. With the canonical `async def stream(req)`
        // + `yield` shape, the call returns the async generator
        // immediately (no awaiting required).
        let py_iter: Py<PyAny> = Python::attach(|py| -> PyResult<Py<PyAny>> {
            let req_value =
                serde_json::to_value(&req).map_err(|e| PyValueError::new_err(e.to_string()))?;
            let req_py = json_value_to_py(py, &req_value)?;
            let result = stream_callable.call1(py, (req_py,))?;
            // Sanity check: must support __anext__.
            let bound = result.bind(py);
            if !bound.hasattr("__anext__")? {
                return Err(PyTypeError::new_err(
                    "python provider stream() must return an async iterator (use `async def stream(...)` with `yield`)",
                ));
            }
            Ok(result)
        })
        .map_err(|e| stream_invalid_err(&self.id, &req.model, format!("dispatch: {e}")))?;

        let id = self.id.clone();
        let model = req.model.clone();

        let s = async_stream::try_stream! {
            loop {
                // Step 2 (GIL): grab the `__anext__()` coroutine and
                // convert to a Rust future. Holding the GIL only long
                // enough to schedule.
                let next_fut = match Python::attach(|py| -> PyResult<_> {
                    let bound = py_iter.bind(py);
                    let coro = bound.call_method0("__anext__")?;
                    pyo3_async_runtimes::tokio::into_future(coro)
                }) {
                    Ok(f) => f,
                    Err(e) => {
                        Err(stream_invalid_err(&id, &model, format!("__anext__: {e}")))?;
                        unreachable!()
                    }
                };

                // Step 3 (no GIL): await the chunk.
                let item = match next_fut.await {
                    Ok(it) => it,
                    Err(e) if Python::attach(|py| e.is_instance_of::<PyStopAsyncIteration>(py)) => {
                        // Clean termination of the async iterator.
                        break;
                    }
                    Err(e) => {
                        Err(stream_invalid_err(&id, &model, format!("await: {e}")))?;
                        unreachable!()
                    }
                };

                // Step 4 (GIL): deserialise the yielded dict to a
                // ChatChunk via the standard kind-tagged JSON shape.
                let chunk: ChatChunk = match Python::attach(|py| -> PyResult<ChatChunk> {
                    let bound = item.into_bound(py);
                    let dict = bound
                        .cast::<PyDict>()
                        .map_err(|_| PyTypeError::new_err(
                            "python provider stream must yield dicts (e.g. {'kind': 'delta', 'text': ...})",
                        ))?;
                    let value = crate::conv::py_to_json(dict.as_any())?;
                    serde_json::from_value::<ChatChunk>(value).map_err(|e| {
                        PyValueError::new_err(format!(
                            "yielded dict does not match ChatChunk schema: {e}",
                        ))
                    })
                }) {
                    Ok(c) => c,
                    Err(e) => {
                        Err(stream_invalid_err(&id, &model, format!("decode: {e}")))?;
                        unreachable!()
                    }
                };
                yield chunk;
            }
        };

        Ok(Box::pin(s))
    }
}

fn stream_invalid_err(provider_id: &str, model: &str, msg: impl Into<String>) -> TakoError {
    TakoError::Provider {
        message: format!("Python provider stream raised: {}", msg.into()),
        source: None,
        details: Box::new(tako_core::ProviderErrorDetails {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            ..Default::default()
        }),
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
    ///
    /// `stream` (Phase 10.D, optional) is `async def stream(request:
    /// dict) -> AsyncIterator[dict]` whose yielded dicts deserialise
    /// to [`ChatChunk`] via the standard `kind`-tagged JSON shape:
    ///
    /// ```python
    /// async def stream(request):
    ///     yield {"kind": "delta", "text": "hello"}
    ///     yield {"kind": "delta", "text": " world"}
    ///     yield {"kind": "end", "finish_reason": "stop",
    ///            "usage": {"input_tokens": 5, "output_tokens": 3}}
    /// ```
    ///
    /// When `stream=` is provided, the provider's
    /// `Capabilities::supports_streaming` flips to `true` so
    /// orchestrators that prefer streaming (e.g. Trinity, AB-MCTS)
    /// will route through the streaming path automatically.
    #[new]
    #[pyo3(signature = (id, chat, stream=None, max_context_tokens=None))]
    fn new(
        id: String,
        chat: Py<PyAny>,
        stream: Option<Py<PyAny>>,
        max_context_tokens: Option<u32>,
    ) -> PyResult<Self> {
        let capabilities = Capabilities {
            max_context_tokens: max_context_tokens.unwrap_or(32_000),
            supports_streaming: stream.is_some(),
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
            stream_callable: stream,
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
