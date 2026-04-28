//! Generic, template-driven HTTP provider.
//!
//! This is the foundation for community-contributed provider adapters. It
//! lets you point [`tako_core::LlmProvider`] at any JSON-over-HTTP endpoint
//! by describing the request body as a `serde_json::Value` template that
//! receives the [`tako_core::ChatRequest`] under the placeholder
//! `{{ request }}`, and the response extraction as a JSON Pointer to the
//! assistant text.
//!
//! Phase 1 ships single-shot only. Streaming will land as a Phase 2 follow
//! once the most common shapes (OpenAI-compatible, Anthropic-compatible,
//! NDJSON) are catalogued.
//!
//! ```no_run
//! use tako_providers_http_generic::{HttpGenericProvider, HttpGenericConfig};
//! use serde_json::json;
//! let cfg = HttpGenericConfig {
//!     id: "cohere:command-r".into(),
//!     model: "command-r".into(),
//!     url: "https://api.cohere.com/v2/chat".into(),
//!     headers: vec![("authorization".into(), "Bearer $COHERE_API_KEY".into())],
//!     body_template: json!({
//!         "model": "{{ model }}",
//!         "messages": "{{ messages }}",
//!     }),
//!     response_text_pointer: "/message/content/0/text".into(),
//!     ..Default::default()
//! };
//! let p = HttpGenericProvider::new(cfg).unwrap();
//! # let _ = p;
//! ```

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, ContentPart, FinishReason, LlmProvider,
    Message, Principal, Role, TakoError, Usage,
};

/// Configuration for an [`HttpGenericProvider`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpGenericConfig {
    pub id: String,
    pub model: String,
    pub url: String,
    /// Headers; values may reference environment variables via the
    /// `$VAR_NAME` syntax. Resolution happens at provider construction.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// JSON template for the request body. The literal strings
    /// `"{{ request }}"`, `"{{ model }}"`, and `"{{ messages }}"` are
    /// replaced with the corresponding fields of the incoming
    /// `ChatRequest`. All other JSON is sent verbatim.
    pub body_template: serde_json::Value,
    /// JSON Pointer (RFC 6901) into the response body that yields the
    /// assistant text.
    pub response_text_pointer: String,
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

impl Default for HttpGenericConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            model: String::new(),
            url: String::new(),
            headers: Vec::new(),
            body_template: serde_json::Value::Null,
            response_text_pointer: "/text".into(),
            capabilities: None,
            timeout_secs: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HttpGenericProvider {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    config: HttpGenericConfig,
    capabilities: Capabilities,
    http: reqwest::Client,
}

impl HttpGenericProvider {
    pub fn new(config: HttpGenericConfig) -> Result<Self, TakoError> {
        if config.id.is_empty() || config.model.is_empty() || config.url.is_empty() {
            return Err(TakoError::Invalid(
                "HttpGenericConfig: id, model, url are required".into(),
            ));
        }

        let mut headers = HeaderMap::new();
        for (k, v) in &config.headers {
            let resolved = resolve_env(v)?;
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| TakoError::Invalid(format!("invalid header name `{k}`: {e}")))?;
            let value = HeaderValue::from_str(&resolved)
                .map_err(|e| TakoError::Invalid(format!("invalid header value for `{k}`: {e}")))?;
            headers.insert(name, value);
        }
        if !headers.contains_key(reqwest::header::CONTENT_TYPE) {
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
        }

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.unwrap_or(120)))
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        let capabilities = config.capabilities.clone().unwrap_or(Capabilities {
            max_context_tokens: 32_000,
            supports_streaming: false,
            supports_tools: false,
            supports_vision: false,
            supports_json_mode: false,
            usd_per_input_mtok: None,
            usd_per_output_mtok: None,
        });

        Ok(Self {
            inner: Arc::new(Inner {
                config,
                capabilities,
                http,
            }),
        })
    }
}

fn resolve_env(value: &str) -> Result<String, TakoError> {
    if let Some(rest) = value.strip_prefix("$") {
        // Allow `$VAR` or `Bearer $VAR` patterns.
        if rest.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            return std::env::var(rest).map_err(|_| {
                TakoError::Invalid(format!("environment variable `{rest}` is not set"))
            });
        }
    }
    if let Some(idx) = value.find('$') {
        let (prefix, suffix) = value.split_at(idx);
        let var = &suffix[1..];
        if var.chars().all(|c| c.is_ascii_uppercase() || c == '_') && !var.is_empty() {
            let resolved = std::env::var(var).map_err(|_| {
                TakoError::Invalid(format!("environment variable `{var}` is not set"))
            })?;
            return Ok(format!("{prefix}{resolved}"));
        }
    }
    Ok(value.to_string())
}

fn render_template(template: &serde_json::Value, req: &ChatRequest) -> serde_json::Value {
    use serde_json::Value;
    match template {
        Value::String(s) => match s.as_str() {
            "{{ request }}" => serde_json::to_value(req).unwrap_or(Value::Null),
            "{{ model }}" => Value::String(req.model.clone()),
            "{{ messages }}" => serde_json::to_value(&req.messages).unwrap_or(Value::Null),
            _ => Value::String(s.clone()),
        },
        Value::Array(arr) => Value::Array(arr.iter().map(|v| render_template(v, req)).collect()),
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), render_template(v, req));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[async_trait]
impl LlmProvider for HttpGenericProvider {
    fn id(&self) -> &str {
        &self.inner.config.id
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
            req.model.clone_from(&self.inner.config.model);
        }
        let body = render_template(&self.inner.config.body_template, &req);
        let resp = self
            .inner
            .http
            .post(&self.inner.config.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let raw = resp.text().await.unwrap_or_default();
            return Err(TakoError::provider(
                self.inner.config.id.clone(),
                self.inner.config.model.clone(),
                format!("HTTP {status}"),
            )
            .with_status(status.as_u16())
            .with_raw_body(raw));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(e.to_string()))?;
        let text = json
            .pointer(&self.inner.config.response_text_pointer)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                TakoError::Invalid(format!(
                    "response_text_pointer `{}` did not resolve to a string",
                    self.inner.config.response_text_pointer
                ))
            })?
            .to_string();

        Ok(ChatResponse {
            message: Message {
                role: Role::Assistant,
                content: vec![ContentPart::Text { text }],
            },
            finish_reason: FinishReason::Stop,
            usage: Usage::default(),
            raw: Default::default(),
        })
    }

    async fn stream(
        &self,
        _principal: &Principal,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        Err(TakoError::Invalid(
            "tako-providers-http-generic does not support streaming yet (Phase 2)".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    #[test]
    fn template_substitutes_placeholders() {
        let req = ChatRequest::new("foo", vec![Message::user("hi")]);
        let tpl = json!({
            "model_field": "{{ model }}",
            "echo": "{{ messages }}",
            "literal": 42,
        });
        let rendered = render_template(&tpl, &req);
        assert_eq!(rendered["model_field"], "foo");
        assert_eq!(rendered["literal"], 42);
        assert!(rendered["echo"].is_array());
    }

    #[test]
    fn missing_env_var_errors() {
        let err = resolve_env("$DEFINITELY_NOT_SET_TAKO_VAR").unwrap_err();
        assert!(matches!(err, TakoError::Invalid(_)));
    }

    #[test]
    fn passthrough_on_no_dollar() {
        assert_eq!(resolve_env("plain-value").unwrap(), "plain-value");
    }
}
