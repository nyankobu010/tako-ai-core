//! Streamable HTTP transport — single-endpoint POST/GET with optional SSE
//! upgrade, per MCP spec 2025-06-18.
//!
//! POSTs carry client→server JSON-RPC frames (requests + notifications)
//! and receive responses inline. The first call to [`notifications`]
//! lazily opens a long-lived `GET {url}` with `Accept: text/event-stream`,
//! parses each SSE `data:` line as JSON-RPC, and broadcasts method-bearing
//! frames (server→client notifications) to subscribers. Frames carrying
//! an `id` (responses to in-flight POSTs) are not delivered through this
//! channel — Streamable HTTP returns them in the POST body itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use futures::stream::BoxStream;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use tako_core::{McpTransport, TakoError};
use tokio::sync::{Mutex, Notify, broadcast};

use crate::jsonrpc::{Response, notification, request};

const NOTIFICATION_BUFFER: usize = 64;

#[derive(Debug)]
struct Inner {
    url: String,
    http: reqwest::Client,
    next_id: AtomicU64,
    session_id: Mutex<Option<String>>,
    notifications: broadcast::Sender<Value>,
    sse_started: AtomicBool,
    cancel: Notify,
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
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
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

        let (tx, _rx) = broadcast::channel(NOTIFICATION_BUFFER);
        Ok(StreamableHttpTransport {
            inner: Arc::new(Inner {
                url,
                http,
                next_id: AtomicU64::new(1),
                session_id: Mutex::new(None),
                notifications: tx,
                sse_started: AtomicBool::new(false),
                cancel: Notify::new(),
            }),
        })
    }
}

impl StreamableHttpTransport {
    /// Spawn the long-lived `GET {url}` SSE reader the first time it is
    /// requested. Subsequent calls are no-ops; the broadcast channel
    /// fans out to every `notifications()` subscriber. The reader exits
    /// on `cancel.notify_waiters()`, server close, or unrecoverable
    /// transport error.
    fn ensure_sse_reader(&self) {
        if self
            .inner
            .sse_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            let session_id = inner.session_id.lock().await.clone();
            let mut req = inner
                .http
                .get(&inner.url)
                .header(reqwest::header::ACCEPT, "text/event-stream");
            if let Some(sid) = session_id.as_ref() {
                req = req.header("Mcp-Session-Id", sid.clone());
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "mcp streamable_http: SSE GET failed");
                    return;
                }
            };
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(%status, %body, "mcp streamable_http: SSE GET non-success");
                return;
            }
            let bytes = resp.bytes_stream();
            let mut events = bytes
                .map(|res| res.map_err(|e| std::io::Error::other(e.to_string())))
                .eventsource()
                .boxed();
            loop {
                tokio::select! {
                    biased;
                    _ = inner.cancel.notified() => {
                        tracing::debug!("mcp streamable_http: SSE reader cancelled");
                        break;
                    }
                    item = events.next() => {
                        let Some(item) = item else { break };
                        let ev = match item {
                            Ok(ev) => ev,
                            Err(e) => {
                                tracing::warn!(error = %e, "mcp streamable_http: SSE read error");
                                break;
                            }
                        };
                        if ev.data.trim().is_empty() {
                            continue;
                        }
                        let parsed: Response = match serde_json::from_str(&ev.data) {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!(error = %e, frame = %ev.data, "mcp streamable_http: invalid JSON-RPC frame");
                                continue;
                            }
                        };
                        // Frames with `id` are responses to in-flight POSTs;
                        // they are returned inline by `request()` and must
                        // not double up on the broadcast channel.
                        if parsed.id.is_some() {
                            continue;
                        }
                        if let Some(method) = parsed.method {
                            let payload = serde_json::json!({
                                "method": method,
                                "params": parsed.params.unwrap_or(Value::Null),
                            });
                            let _ = inner.notifications.send(payload);
                        }
                    }
                }
            }
            tracing::debug!("mcp streamable_http: SSE reader exited");
        });
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

        if let Some(sid) = resp
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok())
        {
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
            return Err(TakoError::Transport(format!(
                "rpc error {}: {}",
                err.code, err.message
            )));
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
        self.ensure_sse_reader();
        let mut rx = self.inner.notifications.subscribe();
        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(item) => yield Ok(item),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
    }

    async fn close(&self) -> Result<(), TakoError> {
        self.inner.cancel.notify_waiters();
        Ok(())
    }
}
