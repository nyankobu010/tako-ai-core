//! Streamable HTTP transport — single-endpoint POST/GET with optional SSE
//! upgrade, per MCP spec 2025-06-18. Phase 1 ships single-shot POST only;
//! the SSE upgrade and `Mcp-Session-Id` lifecycle land in Phase 2.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use tako_core::{McpTransport, TakoError};
use tokio::sync::Mutex;

use crate::jsonrpc::{Response, notification, request};

#[derive(Debug)]
struct Inner {
    url: String,
    http: reqwest::Client,
    next_id: AtomicU64,
    session_id: Mutex<Option<String>>,
}

#[derive(Clone, Debug)]
pub struct StreamableHttpTransport {
    inner: Arc<Inner>,
}

impl StreamableHttpTransport {
    pub fn builder() -> StreamableHttpBuilder {
        StreamableHttpBuilder::default()
    }
}

#[derive(Debug, Default, Clone)]
pub struct StreamableHttpBuilder {
    url: Option<String>,
    headers: HashMap<String, String>,
    timeout: Option<Duration>,
}

impl StreamableHttpBuilder {
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn build(self) -> Result<StreamableHttpTransport, TakoError> {
        let url = self
            .url
            .ok_or_else(|| TakoError::Invalid("StreamableHttpBuilder: url is required".into()))?;
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        for (k, v) in self.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| TakoError::Invalid(format!("invalid header name `{k}`: {e}")))?;
            let value = HeaderValue::from_str(&v)
                .map_err(|e| TakoError::Invalid(format!("invalid header value: {e}")))?;
            headers.insert(name, value);
        }
        let http = reqwest::Client::builder()
            .timeout(self.timeout.unwrap_or(Duration::from_secs(120)))
            .default_headers(headers)
            .build()
            .map_err(|e| TakoError::Transport(e.to_string()))?;

        Ok(StreamableHttpTransport {
            inner: Arc::new(Inner {
                url,
                http,
                next_id: AtomicU64::new(1),
                session_id: Mutex::new(None),
            }),
        })
    }
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn request(&self, method: &str, params: Value) -> Result<Value, TakoError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let body = request(id, method, params);

        let mut req = self.inner.http.post(&self.inner.url).body(body);
        if let Some(sid) = self.inner.session_id.lock().await.as_ref() {
            req = req.header("Mcp-Session-Id", sid.clone());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| TakoError::Transport(format!("post: {e}")))?;

        if let Some(sid) = resp.headers().get("Mcp-Session-Id").and_then(|v| v.to_str().ok()) {
            *self.inner.session_id.lock().await = Some(sid.to_string());
        }

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(TakoError::Transport(format!("HTTP {status}: {body}")));
        }
        let parsed: Response = resp
            .json()
            .await
            .map_err(|e| TakoError::Transport(format!("response parse: {e}")))?;
        if let Some(err) = parsed.error {
            return Err(TakoError::Transport(format!("rpc error {}: {}", err.code, err.message)));
        }
        Ok(parsed.result.unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), TakoError> {
        let body = notification(method, params);
        let mut req = self.inner.http.post(&self.inner.url).body(body);
        if let Some(sid) = self.inner.session_id.lock().await.as_ref() {
            req = req.header("Mcp-Session-Id", sid.clone());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| TakoError::Transport(format!("post: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(TakoError::Transport(format!("HTTP {status}")));
        }
        Ok(())
    }

    async fn notifications(&self) -> BoxStream<'static, Result<Value, TakoError>> {
        // SSE upgrade is Phase 2; meanwhile yield an empty stream.
        Box::pin(futures::stream::empty())
    }

    async fn close(&self) -> Result<(), TakoError> {
        Ok(())
    }
}
