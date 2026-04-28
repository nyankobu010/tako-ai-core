//! Anthropic HTTP client + `LlmProvider` impl.

use std::env;
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

const DEFAULT_VERSION: &str = "2023-06-01";

#[derive(Debug, Default, Clone)]
pub struct AnthropicBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    version: Option<String>,
    timeout: Option<Duration>,
    default_max_tokens: Option<u32>,
    capabilities: Option<Capabilities>,
}

impl AnthropicBuilder {
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Read the API key from the named environment variable. Resolution
    /// happens at `build()` time.
    pub fn api_key_env(mut self, var: impl Into<String>) -> Self {
        self.api_key = Some(format!("$ENV:{}", var.into()));
        self
    }

    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Anthropic requires `max_tokens` on every request. If a `ChatRequest`
    /// has no explicit cap, fall back to this value (default 4096).
    pub fn default_max_tokens(mut self, n: u32) -> Self {
        self.default_max_tokens = Some(n);
        self
    }

    pub fn capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    pub fn build(self) -> Result<AnthropicProvider, TakoError> {
        let model = self
            .model
            .ok_or_else(|| TakoError::Invalid("AnthropicBuilder: model is required".into()))?;
        let api_key = self
            .api_key
            .ok_or_else(|| TakoError::Invalid("AnthropicBuilder: api_key is required".into()))?;
        let api_key = if let Some(var) = api_key.strip_prefix("$ENV:") {
            env::var(var).map_err(|_| {
                TakoError::Invalid(format!("environment variable `{var}` is not set"))
            })?
        } else {
            api_key
        };

        let base_url = self
            .base_url
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        let version = self.version.unwrap_or_else(|| DEFAULT_VERSION.to_string());
        let timeout = self.timeout.unwrap_or(Duration::from_secs(120));

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key)
                .map_err(|e| TakoError::Invalid(format!("invalid api key: {e}")))?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_str(&version)
                .map_err(|e| TakoError::Invalid(format!("invalid version: {e}")))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .timeout(timeout)
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let id = format!("anthropic:{model}");
        let capabilities = self.capabilities.unwrap_or(Capabilities {
            max_context_tokens: 200_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_json_mode: false,
            usd_per_input_mtok: None,
            usd_per_output_mtok: None,
        });

        Ok(AnthropicProvider {
            inner: Arc::new(Inner {
                id,
                model,
                base_url,
                http,
                capabilities,
                default_max_tokens: self.default_max_tokens.unwrap_or(4096),
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
    default_max_tokens: u32,
}

#[derive(Clone, Debug)]
pub struct AnthropicProvider {
    inner: Arc<Inner>,
}

impl AnthropicProvider {
    pub fn builder() -> AnthropicBuilder {
        AnthropicBuilder::default()
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.inner.base_url.trim_end_matches('/'))
    }

    fn map_error(&self, status: reqwest::StatusCode, body: String) -> TakoError {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return TakoError::RateLimited(Duration::from_secs(1));
        }
        TakoError::provider(
            self.inner.id.clone(),
            self.inner.model.clone(),
            format!("HTTP {status}"),
        )
        .with_status(status.as_u16())
        .with_raw_body(body)
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
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
        let body = serde_json::to_value(convert::to_anthropic_request(
            &req,
            self.inner.default_max_tokens,
        ))?;
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
            return Err(self.map_error(status, body));
        }
        let parsed: convert::AnResponse = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        convert::from_anthropic_response(parsed)
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
        let body = serde_json::to_value(convert::to_anthropic_request(
            &req,
            self.inner.default_max_tokens,
        ))?;
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
            return Err(self.map_error(status, body));
        }
        Ok(into_chat_stream(resp))
    }
}
