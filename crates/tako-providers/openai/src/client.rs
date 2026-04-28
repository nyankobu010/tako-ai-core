//! OpenAI HTTP client + `LlmProvider` impl.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, LlmProvider, Principal, TakoError,
};

use crate::convert;
use crate::stream::into_chat_stream;

/// OpenAI provider builder.
#[derive(Debug, Default, Clone)]
pub struct OpenAiBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    timeout: Option<Duration>,
    organization: Option<String>,
    capabilities: Option<Capabilities>,
}

impl OpenAiBuilder {
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Read the API key from the named environment variable. Resolution
    /// happens at `build()` time, not at builder-construction time.
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

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn organization(mut self, org: impl Into<String>) -> Self {
        self.organization = Some(org.into());
        self
    }

    pub fn capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    pub fn build(self) -> Result<OpenAiProvider, TakoError> {
        let model = self
            .model
            .ok_or_else(|| TakoError::Invalid("OpenAiBuilder: model is required".into()))?;
        let api_key = self
            .api_key
            .ok_or_else(|| TakoError::Invalid("OpenAiBuilder: api_key is required".into()))?;
        let api_key = if let Some(var) = api_key.strip_prefix("$ENV:") {
            env::var(var).map_err(|_| {
                TakoError::Invalid(format!("environment variable `{var}` is not set"))
            })?
        } else {
            api_key
        };

        let base_url = self
            .base_url
            .unwrap_or_else(|| "https://api.openai.com".to_string());
        let timeout = self.timeout.unwrap_or(Duration::from_secs(120));

        let mut headers = HeaderMap::new();
        let auth = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|e| TakoError::Invalid(format!("invalid api key: {e}")))?;
        headers.insert(AUTHORIZATION, auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(org) = self.organization {
            headers.insert(
                "OpenAI-Organization",
                HeaderValue::from_str(&org)
                    .map_err(|e| TakoError::Invalid(format!("invalid organization: {e}")))?,
            );
        }

        let http = reqwest::Client::builder()
            .timeout(timeout)
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let id = format!("openai:{model}");
        let capabilities = self.capabilities.unwrap_or(Capabilities {
            max_context_tokens: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_json_mode: true,
            usd_per_input_mtok: None,
            usd_per_output_mtok: None,
        });

        Ok(OpenAiProvider {
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
pub struct OpenAiProvider {
    inner: Arc<Inner>,
}

impl OpenAiProvider {
    pub fn builder() -> OpenAiBuilder {
        OpenAiBuilder::default()
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.inner.base_url.trim_end_matches('/')
        )
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
impl LlmProvider for OpenAiProvider {
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
        let body = serde_json::to_value(convert::to_openai_request(&req))?;
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
        let parsed: convert::OaResponse = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        convert::from_openai_response(parsed)
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
        let body = serde_json::to_value(convert::to_openai_request(&req))?;
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
