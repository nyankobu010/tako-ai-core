//! Provider `#[pyclass]`es: PyOpenAI, PyAnthropic, PyFakeProvider.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_core::{Capabilities, ChatChunk, ChatRequest, ChatResponse, FinishReason, LlmProvider, Message, Principal, Role, TakoError, Usage};
use tako_providers_anthropic::AnthropicProvider;
use tako_providers_openai::OpenAiProvider;

/// Internal handle every Python provider class wraps. Cloneable so we can
/// hand a fresh Arc to `PyOrchestrator` without retaining a Python ref.
#[derive(Clone)]
pub struct ProviderHandle {
    pub inner: Arc<dyn LlmProvider>,
}

impl std::fmt::Debug for ProviderHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderHandle").field("id", &self.inner.id()).finish()
    }
}

#[pyclass(name = "OpenAI", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyOpenAI {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyOpenAI {
    /// Construct an OpenAI chat.completions provider.
    ///
    /// `api_key` is required. Pass either the literal key, or a value
    /// like `"$ENV:OPENAI_API_KEY"` to read from the environment.
    #[new]
    #[pyo3(signature = (model, api_key, base_url=None, timeout_secs=None, organization=None))]
    fn new(model: &str, api_key: &str, base_url: Option<&str>, timeout_secs: Option<u64>, organization: Option<&str>) -> PyResult<Self> {
        let mut b = OpenAiProvider::builder().api_key(api_key).model(model);
        if let Some(u) = base_url {
            b = b.base_url(u);
        }
        if let Some(t) = timeout_secs {
            b = b.timeout(Duration::from_secs(t));
        }
        if let Some(o) = organization {
            b = b.organization(o);
        }
        let p = b.build().map_err(map_err)?;
        Ok(Self {
            handle: ProviderHandle { inner: Arc::new(p) },
        })
    }

    fn id(&self) -> &str {
        self.handle.inner.id()
    }
}

#[pyclass(name = "Anthropic", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyAnthropic {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyAnthropic {
    #[new]
    #[pyo3(signature = (model, api_key, base_url=None, timeout_secs=None, default_max_tokens=None))]
    fn new(model: &str, api_key: &str, base_url: Option<&str>, timeout_secs: Option<u64>, default_max_tokens: Option<u32>) -> PyResult<Self> {
        let mut b = AnthropicProvider::builder().api_key(api_key).model(model);
        if let Some(u) = base_url {
            b = b.base_url(u);
        }
        if let Some(t) = timeout_secs {
            b = b.timeout(Duration::from_secs(t));
        }
        if let Some(n) = default_max_tokens {
            b = b.default_max_tokens(n);
        }
        let p = b.build().map_err(map_err)?;
        Ok(Self {
            handle: ProviderHandle { inner: Arc::new(p) },
        })
    }

    fn id(&self) -> &str {
        self.handle.inner.id()
    }
}

/// Fake provider for tests. Returns canned text and counts calls so
/// concurrency / smoke tests can assert on it.
#[pyclass(name = "FakeProvider", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyFakeProvider {
    pub handle: ProviderHandle,
    inner: Arc<FakeInner>,
}

impl std::fmt::Debug for FakeInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeInner")
            .field("id", &self.id)
            .field("calls", &self.calls.load(std::sync::atomic::Ordering::Relaxed))
            .finish()
    }
}

struct FakeInner {
    id: String,
    canned_text: String,
    delay_ms: u64,
    calls: AtomicUsize,
    capabilities: Capabilities,
}

#[async_trait]
impl LlmProvider for FakeInner {
    fn id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    async fn chat(&self, _principal: &Principal, _req: ChatRequest) -> Result<ChatResponse, TakoError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        Ok(ChatResponse {
            message: Message {
                role: Role::Assistant,
                content: vec![tako_core::ContentPart::Text { text: self.canned_text.clone() }],
            },
            finish_reason: FinishReason::Stop,
            usage: Usage { input_tokens: 1, output_tokens: 1 },
            raw: Default::default(),
        })
    }
    async fn stream(&self, _p: &Principal, _r: ChatRequest) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        Err(TakoError::Invalid("FakeProvider streaming not supported".into()))
    }
}

#[pymethods]
impl PyFakeProvider {
    #[new]
    #[pyo3(signature = (canned_text="ok", id="fake:test", delay_ms=0))]
    fn new(canned_text: &str, id: &str, delay_ms: u64) -> Self {
        let inner = Arc::new(FakeInner {
            id: id.to_string(),
            canned_text: canned_text.to_string(),
            delay_ms,
            calls: AtomicUsize::new(0),
            capabilities: Capabilities::default(),
        });
        Self {
            handle: ProviderHandle { inner: inner.clone() as Arc<dyn LlmProvider> },
            inner,
        }
    }

    fn id(&self) -> &str {
        &self.inner.id
    }

    fn call_count(&self) -> usize {
        self.inner.calls.load(Ordering::SeqCst)
    }
}

pub(crate) fn map_err(e: TakoError) -> PyErr {
    PyValueError::new_err(e.to_string())
}
