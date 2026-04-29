//! Vertex AI HTTP client + `LlmProvider` impl.

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

/// Default GCP location/region for Vertex AI.
const DEFAULT_LOCATION: &str = "us-central1";

/// Vertex AI provider builder.
#[derive(Debug, Default, Clone)]
pub struct VertexBuilder {
    access_token: Option<String>,
    project_id: Option<String>,
    location: Option<String>,
    model: Option<String>,
    endpoint_url: Option<String>,
    timeout: Option<Duration>,
    capabilities: Option<Capabilities>,
}

impl VertexBuilder {
    /// Set a pre-resolved OAuth2 access token. The provider does not refresh
    /// it; callers using long-lived providers should rebuild the provider
    /// when their token nears expiry, or override the
    /// [`endpoint_url`](Self::endpoint_url) to a local proxy that injects
    /// fresh credentials.
    pub fn access_token(mut self, token: impl Into<String>) -> Self {
        self.access_token = Some(token.into());
        self
    }

    /// Read the access token from the named environment variable. Resolution
    /// happens at `build()` time.
    pub fn access_token_env(mut self, var: impl Into<String>) -> Self {
        self.access_token = Some(format!("$ENV:{}", var.into()));
        self
    }

    pub fn project_id(mut self, id: impl Into<String>) -> Self {
        self.project_id = Some(id.into());
        self
    }

    /// GCP location/region. Defaults to `us-central1`.
    pub fn location(mut self, loc: impl Into<String>) -> Self {
        self.location = Some(loc.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Override the Vertex AI endpoint. Useful for tests (point at wiremock)
    /// or VPC-private endpoints. When set, replaces the default
    /// `{location}-aiplatform.googleapis.com` host.
    pub fn endpoint_url(mut self, url: impl Into<String>) -> Self {
        self.endpoint_url = Some(url.into());
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

    pub fn build(self) -> Result<VertexProvider, TakoError> {
        let project_id = self
            .project_id
            .ok_or_else(|| TakoError::Invalid("VertexBuilder: project_id is required".into()))?;
        let model = self
            .model
            .ok_or_else(|| TakoError::Invalid("VertexBuilder: model is required".into()))?;
        let access_token = self
            .access_token
            .ok_or_else(|| TakoError::Invalid("VertexBuilder: access_token is required".into()))?;
        let access_token = if let Some(var) = access_token.strip_prefix("$ENV:") {
            env::var(var).map_err(|_| {
                TakoError::Invalid(format!("environment variable `{var}` is not set"))
            })?
        } else {
            access_token
        };

        let location = self.location.unwrap_or_else(|| DEFAULT_LOCATION.to_string());
        let timeout = self.timeout.unwrap_or(Duration::from_secs(120));

        let mut headers = HeaderMap::new();
        let auth = HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|e| TakoError::Invalid(format!("invalid access token: {e}")))?;
        headers.insert(AUTHORIZATION, auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .timeout(timeout)
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let id = format!("vertex:{model}");
        let capabilities = self.capabilities.unwrap_or(Capabilities {
            max_context_tokens: 1_000_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_json_mode: true,
            usd_per_input_mtok: None,
            usd_per_output_mtok: None,
        });

        Ok(VertexProvider {
            inner: Arc::new(Inner {
                id,
                model,
                project_id,
                location,
                endpoint_url: self.endpoint_url,
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
    project_id: String,
    location: String,
    endpoint_url: Option<String>,
    http: reqwest::Client,
    capabilities: Capabilities,
}

#[derive(Clone, Debug)]
pub struct VertexProvider {
    inner: Arc<Inner>,
}

impl VertexProvider {
    pub fn builder() -> VertexBuilder {
        VertexBuilder::default()
    }

    fn base_endpoint(&self) -> String {
        match &self.inner.endpoint_url {
            Some(u) => u.trim_end_matches('/').to_string(),
            None => format!(
                "https://{}-aiplatform.googleapis.com",
                self.inner.location
            ),
        }
    }

    fn url_for(&self, action: &str, query: Option<&str>) -> String {
        let base = self.base_endpoint();
        let path = format!(
            "/v1/projects/{}/locations/{}/publishers/google/models/{}:{}",
            self.inner.project_id, self.inner.location, self.inner.model, action,
        );
        match query {
            Some(q) => format!("{base}{path}?{q}"),
            None => format!("{base}{path}"),
        }
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
impl LlmProvider for VertexProvider {
    fn id(&self) -> &str {
        &self.inner.id
    }

    fn capabilities(&self) -> &Capabilities {
        &self.inner.capabilities
    }

    async fn chat(
        &self,
        _principal: &Principal,
        req: ChatRequest,
    ) -> Result<ChatResponse, TakoError> {
        let body = serde_json::to_value(convert::to_vertex_request(&req))?;
        let resp = self
            .inner
            .http
            .post(self.url_for("generateContent", None))
            .json(&body)
            .send()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(self.map_error(status, body).await);
        }
        let parsed: convert::VxResponse = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        convert::from_vertex_response(parsed)
    }

    async fn stream(
        &self,
        _principal: &Principal,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        let body = serde_json::to_value(convert::to_vertex_request(&req))?;
        let resp = self
            .inner
            .http
            .post(self.url_for("streamGenerateContent", Some("alt=sse")))
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
