//! Azure OpenAI HTTP client + `LlmProvider` impl.
//!
//! Reuses the OpenAI request/response/SSE conversion modules verbatim — Azure's
//! wire body is byte-identical. Only the URL shape and auth header differ.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, LlmProvider, Principal, TakoError,
};
use tako_providers_openai::convert;
use tako_providers_openai::stream::into_chat_stream;

/// Default Azure OpenAI REST API version.
///
/// Track the `api-version` value Azure recommends in their docs; the provider
/// is configurable via [`AzureOpenAiBuilder::api_version`] if a user needs a
/// newer/older one.
const DEFAULT_API_VERSION: &str = "2024-10-21";

/// Azure OpenAI provider builder.
#[derive(Debug, Default, Clone)]
pub struct AzureOpenAiBuilder {
    api_key: Option<String>,
    endpoint: Option<String>,
    deployment: Option<String>,
    api_version: Option<String>,
    timeout: Option<Duration>,
    capabilities: Option<Capabilities>,
}

impl AzureOpenAiBuilder {
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

    /// Azure OpenAI resource endpoint, e.g.
    /// `https://my-resource.openai.azure.com`.
    pub fn endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = Some(url.into());
        self
    }

    /// Azure deployment name (a user-defined alias mapping to a model). The
    /// provider id surfaced upstream becomes `azure-openai:<deployment>`.
    pub fn deployment(mut self, name: impl Into<String>) -> Self {
        self.deployment = Some(name.into());
        self
    }

    /// REST API version. Defaults to [`DEFAULT_API_VERSION`].
    pub fn api_version(mut self, v: impl Into<String>) -> Self {
        self.api_version = Some(v.into());
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

    pub fn build(self) -> Result<AzureOpenAiProvider, TakoError> {
        let deployment = self.deployment.ok_or_else(|| {
            TakoError::Invalid("AzureOpenAiBuilder: deployment is required".into())
        })?;
        let endpoint = self
            .endpoint
            .ok_or_else(|| TakoError::Invalid("AzureOpenAiBuilder: endpoint is required".into()))?;
        let api_key = self
            .api_key
            .ok_or_else(|| TakoError::Invalid("AzureOpenAiBuilder: api_key is required".into()))?;
        let api_key = if let Some(var) = api_key.strip_prefix("$ENV:") {
            env::var(var).map_err(|_| {
                TakoError::Invalid(format!("environment variable `{var}` is not set"))
            })?
        } else {
            api_key
        };

        let api_version = self
            .api_version
            .unwrap_or_else(|| DEFAULT_API_VERSION.to_string());
        let timeout = self.timeout.unwrap_or(Duration::from_secs(120));

        let mut headers = HeaderMap::new();
        let key = HeaderValue::from_str(&api_key)
            .map_err(|e| TakoError::Invalid(format!("invalid api key: {e}")))?;
        headers.insert("api-key", key);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .timeout(timeout)
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let id = format!("azure-openai:{deployment}");
        let capabilities = self.capabilities.unwrap_or(Capabilities {
            max_context_tokens: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_json_mode: true,
            usd_per_input_mtok: None,
            usd_per_output_mtok: None,
        });

        Ok(AzureOpenAiProvider {
            inner: Arc::new(Inner {
                id,
                deployment,
                endpoint,
                api_version,
                http,
                capabilities,
            }),
        })
    }
}

#[derive(Debug)]
struct Inner {
    id: String,
    deployment: String,
    endpoint: String,
    api_version: String,
    http: reqwest::Client,
    capabilities: Capabilities,
}

#[derive(Clone, Debug)]
pub struct AzureOpenAiProvider {
    inner: Arc<Inner>,
}

impl AzureOpenAiProvider {
    pub fn builder() -> AzureOpenAiBuilder {
        AzureOpenAiBuilder::default()
    }

    fn endpoint_url(&self) -> String {
        format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            self.inner.endpoint.trim_end_matches('/'),
            self.inner.deployment,
            self.inner.api_version,
        )
    }

    async fn map_error(&self, status: reqwest::StatusCode, body: String) -> TakoError {
        let mut err = TakoError::provider(
            self.inner.id.clone(),
            self.inner.deployment.clone(),
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
impl LlmProvider for AzureOpenAiProvider {
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
        // Azure routes by deployment, not model — but the wire body still
        // requires a `model` field (Azure ignores its value for routing).
        if req.model.is_empty() {
            req.model.clone_from(&self.inner.deployment);
        }
        req.stream = false;
        let body = serde_json::to_value(convert::to_openai_request(&req))?;
        let resp = self
            .inner
            .http
            .post(self.endpoint_url())
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
            req.model.clone_from(&self.inner.deployment);
        }
        req.stream = true;
        let body = serde_json::to_value(convert::to_openai_request(&req))?;
        let resp = self
            .inner
            .http
            .post(self.endpoint_url())
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
