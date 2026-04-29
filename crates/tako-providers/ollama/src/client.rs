//! Ollama HTTP client + `LlmProvider` impl.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, LlmProvider, Principal, TakoError,
};

use crate::convert;
use crate::stream::into_chat_stream;

#[derive(Debug, Default, Clone)]
pub struct OllamaBuilder {
    base_url: Option<String>,
    model: Option<String>,
    timeout: Option<Duration>,
    capabilities: Option<Capabilities>,
}

impl OllamaBuilder {
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    pub fn build(self) -> Result<OllamaProvider, TakoError> {
        let model = self
            .model
            .ok_or_else(|| TakoError::Invalid("OllamaBuilder: model is required".into()))?;
        let base_url = self
            .base_url
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        // Local-runner inference can be slow; default timeout is generous.
        let timeout = self.timeout.unwrap_or(Duration::from_secs(600));

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .timeout(timeout)
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let id = format!("ollama:{model}");
        let capabilities = self.capabilities.unwrap_or(Capabilities {
            // Local models vary wildly; pick a conservative default.
            max_context_tokens: 8_192,
            supports_streaming: true,
            // Ollama tool-call support landed in 0.3; assume true and
            // let users override via the builder if their model lacks it.
            supports_tools: true,
            supports_vision: false,
            supports_json_mode: true,
            // Local-runner — no per-token billing.
            usd_per_input_mtok: Some(0.0),
            usd_per_output_mtok: Some(0.0),
        });

        Ok(OllamaProvider {
            inner: Arc::new(Inner {
                id,
                model,
                base_url,
                http,
                capabilities,
            }),
        })
    }
}

#[derive(Debug)]
struct Inner {
    id: String,
    model: String,
    base_url: String,
    http: reqwest::Client,
    capabilities: Capabilities,
}

#[derive(Clone, Debug)]
pub struct OllamaProvider {
    inner: Arc<Inner>,
}

impl OllamaProvider {
    pub fn builder() -> OllamaBuilder {
        OllamaBuilder::default()
    }

    fn endpoint(&self) -> String {
        format!("{}/api/chat", self.inner.base_url.trim_end_matches('/'))
    }

    async fn map_error(&self, status: reqwest::StatusCode, body: String) -> TakoError {
        let mut err = TakoError::provider(
            self.inner.id.clone(),
            self.inner.model.clone(),
            format!("HTTP {status}"),
        )
        .with_status(status.as_u16())
        .with_raw_body(body);
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            err = TakoError::RateLimited(Duration::from_secs(1));
        }
        err
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn id(&self) -> &str {
        &self.inner.id
    }

    fn capabilities(&self) -> &Capabilities {
        &self.inner.capabilities
    }

    async fn chat(
        &self,
        _principal: &Principal,
        mut req: ChatRequest,
    ) -> Result<ChatResponse, TakoError> {
        if req.model.is_empty() {
            req.model.clone_from(&self.inner.model);
        }
        req.stream = false;
        let body = serde_json::to_value(convert::to_ollama_request(&req))?;
        let resp = self
            .inner
            .http
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(self.map_error(status, body).await);
        }
        let parsed: convert::OlResponse = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        convert::from_ollama_response(parsed)
    }

    async fn stream(
        &self,
        _principal: &Principal,
        mut req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        if req.model.is_empty() {
            req.model.clone_from(&self.inner.model);
        }
        req.stream = true;
        let body = serde_json::to_value(convert::to_ollama_request(&req))?;
        let resp = self
            .inner
            .http
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(self.map_error(status, body).await);
        }
        Ok(into_chat_stream(resp))
    }
}
