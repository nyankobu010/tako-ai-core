//! Generic, template-driven HTTP provider.
//!
//! This is the foundation for community-contributed provider adapters. It
//! lets you point [`tako_core::LlmProvider`] at any JSON-over-HTTP endpoint
//! by describing the request body as a `serde_json::Value` template that
//! receives the [`tako_core::ChatRequest`] under the placeholder
//! `{{ request }}`, and the response extraction as a JSON Pointer to the
//! assistant text.
//!
//! Streaming is opt-in (Phase 11.B): set
//! [`HttpGenericConfig::stream_config`] to either
//! [`StreamConfig::OpenAiSse`] (OpenAI-compatible `event:` + `data:`
//! frames terminated by `[DONE]`) or [`StreamConfig::NdJson`] (one JSON
//! frame per newline). Both variants extract the content delta, finish
//! reason, and usage via JSON Pointer (RFC 6901), so any endpoint that
//! emits structured frames can be configured without code changes.
//! Tool-call delta extraction is intentionally out of scope (operator
//! shapes vary too widely); operators streaming tool calls should use
//! the OpenAI provider's typed parser.
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
use eventsource_stream::Eventsource;
use futures::stream::{BoxStream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, ContentPart, FinishReason, LlmProvider,
    Message, Principal, Role, TakoError, Usage,
};
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;

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
    /// Phase 11.B — streaming wire shape. `None` (the default) means
    /// the endpoint does not stream and `LlmProvider::stream` will
    /// return `TakoError::Invalid`. When set, `Capabilities::supports_streaming`
    /// is automatically `true` (unless an operator-supplied
    /// [`HttpGenericConfig::capabilities`] overrides it).
    #[serde(default)]
    pub stream_config: Option<StreamConfig>,
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
            stream_config: None,
        }
    }
}

/// Phase 11.B — wire-format selector for [`HttpGenericProvider::stream`].
///
/// Both variants extract the content delta, finish reason, and usage
/// via JSON Pointer (RFC 6901) over each parsed frame, so operators
/// can target endpoints with arbitrary JSON shapes by tweaking the
/// pointer strings. `tool_calls` are not extracted — operators
/// streaming tool calls must use the OpenAI provider's typed parser.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamConfig {
    /// OpenAI-compatible Server-Sent Events. Each `event:` carries a
    /// JSON `data:` line; the stream terminates on `data: [DONE]`.
    #[serde(rename = "openai_sse")]
    OpenAiSse {
        /// JSON Pointer into each parsed frame for the content delta
        /// string. Defaults to OpenAI's `/choices/0/delta/content`.
        #[serde(default = "default_oa_content_pointer")]
        content_pointer: String,
        /// JSON Pointer to the per-frame finish reason. Resolves to a
        /// string (`"stop"` / `"length"` / etc.) on the final frame.
        /// Defaults to `/choices/0/finish_reason`.
        #[serde(default = "default_oa_finish_pointer")]
        finish_reason_pointer: String,
        /// Optional JSON Pointer to a per-frame usage object. When
        /// resolvable, the `input_tokens` / `output_tokens` are merged
        /// into the final `ChatChunk::End`. Defaults to
        /// `Some("/usage")`; explicit `None` disables extraction.
        #[serde(default = "default_oa_usage_pointer")]
        usage_pointer: Option<String>,
    },
    /// Newline-delimited JSON: one full frame per `\n`. Termination on
    /// EOF or on a frame whose `finish_reason_pointer` resolves to a
    /// non-null string.
    #[serde(rename = "ndjson")]
    NdJson {
        content_pointer: String,
        finish_reason_pointer: String,
        #[serde(default)]
        usage_pointer: Option<String>,
    },
}

fn default_oa_content_pointer() -> String {
    "/choices/0/delta/content".into()
}

fn default_oa_finish_pointer() -> String {
    "/choices/0/finish_reason".into()
}

fn default_oa_usage_pointer() -> Option<String> {
    Some("/usage".into())
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

        // Phase 11.B — when the operator hasn't supplied an explicit
        // `Capabilities`, derive `supports_streaming` from whether a
        // `stream_config` is present. An explicit override always wins.
        let capabilities = config.capabilities.clone().unwrap_or(Capabilities {
            max_context_tokens: 32_000,
            supports_streaming: config.stream_config.is_some(),
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
        mut req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        let cfg = self.inner.config.stream_config.clone().ok_or_else(|| {
            TakoError::Invalid(
                "tako-providers-http-generic: no stream_config set on this provider; \
                 set HttpGenericConfig::stream_config to OpenAiSse or NdJson to enable streaming"
                    .into(),
            )
        })?;
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

        Ok(match cfg {
            StreamConfig::OpenAiSse {
                content_pointer,
                finish_reason_pointer,
                usage_pointer,
            } => stream_openai_sse(resp, content_pointer, finish_reason_pointer, usage_pointer),
            StreamConfig::NdJson {
                content_pointer,
                finish_reason_pointer,
                usage_pointer,
            } => stream_ndjson(resp, content_pointer, finish_reason_pointer, usage_pointer),
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 11.B — JSON-pointer-based extractors shared by both stream
// adapters. `serde_json::Value::pointer` is RFC 6901-compliant so
// operators can target arbitrary endpoint shapes without code changes.
// ---------------------------------------------------------------------------

fn resolve_str(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn resolve_finish(value: &serde_json::Value, pointer: &str) -> Option<FinishReason> {
    let s = value.pointer(pointer).and_then(|v| v.as_str())?;
    Some(match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    })
}

fn resolve_usage(value: &serde_json::Value, pointer: &str) -> Option<Usage> {
    let usage = value.pointer(pointer)?;
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)?;
    let output = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)?;
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
    })
}

fn stream_openai_sse(
    resp: reqwest::Response,
    content_pointer: String,
    finish_reason_pointer: String,
    usage_pointer: Option<String>,
) -> BoxStream<'static, Result<ChatChunk, TakoError>> {
    let bytes = resp.bytes_stream();
    let events = bytes
        .map(|res| res.map_err(|e| std::io::Error::other(e.to_string())))
        .eventsource();
    let stream = async_stream::stream! {
        let mut last_finish: Option<FinishReason> = None;
        let mut last_usage = Usage::default();
        let mut events = Box::pin(events);
        while let Some(item) = events.next().await {
            match item {
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("{e}") });
                    last_finish = Some(FinishReason::Error);
                    break;
                }
                Ok(ev) => {
                    if ev.data == "[DONE]" {
                        break;
                    }
                    let frame: serde_json::Value = match serde_json::from_str(&ev.data) {
                        Ok(v) => v,
                        Err(e) => {
                            yield Ok(ChatChunk::Error { message: format!("invalid frame: {e}") });
                            continue;
                        }
                    };
                    if let Some(reason) = resolve_finish(&frame, &finish_reason_pointer) {
                        last_finish = Some(reason);
                    }
                    if let Some(ptr) = usage_pointer.as_deref()
                        && let Some(u) = resolve_usage(&frame, ptr)
                    {
                        last_usage = u;
                    }
                    if let Some(text) = resolve_str(&frame, &content_pointer) {
                        yield Ok(ChatChunk::Delta { text: Some(text), tool_calls: vec![] });
                    }
                }
            }
        }
        yield Ok(ChatChunk::End {
            finish_reason: last_finish.unwrap_or(FinishReason::Other),
            usage: last_usage,
        });
    };
    stream.boxed()
}

fn stream_ndjson(
    resp: reqwest::Response,
    content_pointer: String,
    finish_reason_pointer: String,
    usage_pointer: Option<String>,
) -> BoxStream<'static, Result<ChatChunk, TakoError>> {
    let byte_stream = resp
        .bytes_stream()
        .map(|res| res.map_err(|e| std::io::Error::other(e.to_string())));
    let reader = StreamReader::new(byte_stream);
    let lines = FramedRead::new(reader, LinesCodec::new());
    let stream = async_stream::stream! {
        let mut last_finish: Option<FinishReason> = None;
        let mut last_usage = Usage::default();
        let mut lines = Box::pin(lines);
        while let Some(item) = lines.next().await {
            let line = match item {
                Ok(s) => s,
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("{e}") });
                    last_finish = Some(FinishReason::Error);
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let frame: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    yield Ok(ChatChunk::Error { message: format!("invalid frame: {e}") });
                    continue;
                }
            };
            if let Some(reason) = resolve_finish(&frame, &finish_reason_pointer) {
                last_finish = Some(reason);
            }
            if let Some(ptr) = usage_pointer.as_deref()
                && let Some(u) = resolve_usage(&frame, ptr)
            {
                last_usage = u;
            }
            if let Some(text) = resolve_str(&frame, &content_pointer) {
                yield Ok(ChatChunk::Delta { text: Some(text), tool_calls: vec![] });
            }
            // NDJSON terminates as soon as a frame's finish reason
            // resolves — there is no `[DONE]` sentinel.
            if last_finish.is_some() {
                break;
            }
        }
        yield Ok(ChatChunk::End {
            finish_reason: last_finish.unwrap_or(FinishReason::Other),
            usage: last_usage,
        });
    };
    stream.boxed()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

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

    #[test]
    fn stream_config_serialises_openai_sse_round_trip() {
        let cfg = StreamConfig::OpenAiSse {
            content_pointer: "/x".into(),
            finish_reason_pointer: "/y".into(),
            usage_pointer: Some("/z".into()),
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["kind"], "openai_sse");
        let decoded: StreamConfig = serde_json::from_value(v).unwrap();
        match decoded {
            StreamConfig::OpenAiSse {
                content_pointer,
                finish_reason_pointer,
                usage_pointer,
            } => {
                assert_eq!(content_pointer, "/x");
                assert_eq!(finish_reason_pointer, "/y");
                assert_eq!(usage_pointer.as_deref(), Some("/z"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn stream_config_serialises_ndjson_round_trip() {
        let cfg = StreamConfig::NdJson {
            content_pointer: "/text".into(),
            finish_reason_pointer: "/done".into(),
            usage_pointer: None,
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["kind"], "ndjson");
        let _: StreamConfig = serde_json::from_value(v).unwrap();
    }

    #[test]
    fn default_pointers_match_openai_layout() {
        assert_eq!(default_oa_content_pointer(), "/choices/0/delta/content");
        assert_eq!(default_oa_finish_pointer(), "/choices/0/finish_reason");
        assert_eq!(default_oa_usage_pointer().as_deref(), Some("/usage"));
    }

    #[test]
    fn capability_flag_set_when_stream_config_is_some() {
        let p = HttpGenericProvider::new(HttpGenericConfig {
            id: "x".into(),
            model: "m".into(),
            url: "https://example.invalid".into(),
            stream_config: Some(StreamConfig::OpenAiSse {
                content_pointer: default_oa_content_pointer(),
                finish_reason_pointer: default_oa_finish_pointer(),
                usage_pointer: None,
            }),
            ..Default::default()
        })
        .unwrap();
        assert!(p.capabilities().supports_streaming);
    }

    #[test]
    fn capability_flag_unset_when_stream_config_is_none() {
        let p = HttpGenericProvider::new(HttpGenericConfig {
            id: "x".into(),
            model: "m".into(),
            url: "https://example.invalid".into(),
            stream_config: None,
            ..Default::default()
        })
        .unwrap();
        assert!(!p.capabilities().supports_streaming);
    }

    #[test]
    fn operator_capability_override_wins_over_stream_config_inference() {
        let p = HttpGenericProvider::new(HttpGenericConfig {
            id: "x".into(),
            model: "m".into(),
            url: "https://example.invalid".into(),
            // No stream_config — but operator forces supports_streaming.
            stream_config: None,
            capabilities: Some(Capabilities {
                max_context_tokens: 1,
                supports_streaming: true,
                supports_tools: false,
                supports_vision: false,
                supports_json_mode: false,
                usd_per_input_mtok: None,
                usd_per_output_mtok: None,
            }),
            ..Default::default()
        })
        .unwrap();
        assert!(p.capabilities().supports_streaming);
    }

    #[test]
    fn resolve_str_handles_empty_and_missing() {
        let v = serde_json::json!({"a": "hi", "b": "", "c": null});
        assert_eq!(resolve_str(&v, "/a").as_deref(), Some("hi"));
        assert!(resolve_str(&v, "/b").is_none());
        assert!(resolve_str(&v, "/c").is_none());
        assert!(resolve_str(&v, "/missing").is_none());
    }

    #[test]
    fn resolve_finish_maps_known_strings() {
        let v = serde_json::json!({"r": "stop"});
        assert_eq!(resolve_finish(&v, "/r"), Some(FinishReason::Stop));
        let v = serde_json::json!({"r": "length"});
        assert_eq!(resolve_finish(&v, "/r"), Some(FinishReason::Length));
        let v = serde_json::json!({"r": "weird"});
        assert_eq!(resolve_finish(&v, "/r"), Some(FinishReason::Other));
        let v = serde_json::json!({});
        assert!(resolve_finish(&v, "/r").is_none());
    }

    #[test]
    fn resolve_usage_supports_both_naming_conventions() {
        let v = serde_json::json!({"usage": {"input_tokens": 12, "output_tokens": 7}});
        assert_eq!(
            resolve_usage(&v, "/usage"),
            Some(Usage {
                input_tokens: 12,
                output_tokens: 7,
            })
        );
        let v = serde_json::json!({"usage": {"prompt_tokens": 4, "completion_tokens": 9}});
        assert_eq!(
            resolve_usage(&v, "/usage"),
            Some(Usage {
                input_tokens: 4,
                output_tokens: 9,
            })
        );
        let v = serde_json::json!({});
        assert!(resolve_usage(&v, "/usage").is_none());
    }
}
