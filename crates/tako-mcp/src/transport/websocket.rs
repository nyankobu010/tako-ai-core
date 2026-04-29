//! WebSocket MCP transport.
//!
//! Bidirectional JSON-RPC over a single WebSocket connection: the
//! client sends requests/notifications as text frames; the server
//! sends responses (matched to pending requests by `id`) and
//! notifications (no `id`) back over the same socket.
//!
//! Behaviour mirrors [`super::stdio::StdioTransport`]: a background
//! reader task demultiplexes incoming frames into per-request oneshots
//! and a `broadcast` channel for notifications.
//!
//! Gated behind the `ws` Cargo feature so `tokio-tungstenite` only
//! arrives in the dep tree when explicitly enabled.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures::SinkExt;
use futures::stream::{BoxStream, SplitSink, StreamExt};
use serde_json::Value;
use tako_core::{McpTransport, TakoError};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::jsonrpc::{Response, notification, request};

const NOTIFICATION_BUFFER: usize = 64;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = SplitSink<WsStream, Message>;

#[derive(Debug)]
struct Inner {
    next_id: AtomicU64,
    sink: Mutex<Option<WsSink>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, TakoError>>>>,
    notifications: broadcast::Sender<Value>,
}

/// MCP WebSocket transport.
#[derive(Clone, Debug)]
pub struct WebSocketTransport {
    inner: Arc<Inner>,
}

impl WebSocketTransport {
    /// Connect to `url` (e.g. `ws://localhost:8080/mcp` or
    /// `wss://example.com/mcp`) and start the demuxing reader task.
    pub async fn connect(url: &str) -> Result<Self, TakoError> {
        let (ws, _resp) = connect_async(url)
            .await
            .map_err(|e| TakoError::Transport(format!("ws connect: {e}")))?;
        let (sink, mut source) = ws.split();

        let (tx, _rx) = broadcast::channel(NOTIFICATION_BUFFER);
        let inner = Arc::new(Inner {
            next_id: AtomicU64::new(1),
            sink: Mutex::new(Some(sink)),
            pending: Mutex::new(HashMap::new()),
            notifications: tx,
        });

        // Reader task: parse one JSON-RPC frame per text message and
        // dispatch.
        let inner_reader = Arc::clone(&inner);
        tokio::spawn(async move {
            while let Some(msg) = source.next().await {
                let text = match msg {
                    Ok(Message::Text(t)) => t.to_string(),
                    Ok(Message::Binary(b)) => match std::str::from_utf8(&b) {
                        Ok(s) => s.to_string(),
                        Err(e) => {
                            tracing::warn!(error = %e, "mcp ws: non-UTF8 binary frame");
                            continue;
                        }
                    },
                    Ok(Message::Close(_)) => break,
                    Ok(_) => continue, // ping/pong/etc.
                    Err(e) => {
                        tracing::warn!(error = %e, "mcp ws: read error");
                        break;
                    }
                };
                if text.trim().is_empty() {
                    continue;
                }
                let parsed: Response = match serde_json::from_str(&text) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, frame = %text, "mcp ws: invalid JSON-RPC frame");
                        continue;
                    }
                };
                if let Some(id) = parsed.id {
                    if let Some(tx) = inner_reader.pending.lock().await.remove(&id) {
                        let result = if let Some(err) = parsed.error {
                            Err(TakoError::Transport(format!(
                                "rpc error {}: {}",
                                err.code, err.message
                            )))
                        } else {
                            Ok(parsed.result.unwrap_or(Value::Null))
                        };
                        let _ = tx.send(result);
                    }
                } else if let Some(method) = parsed.method {
                    let payload = serde_json::json!({
                        "method": method,
                        "params": parsed.params.unwrap_or(Value::Null),
                    });
                    let _ = inner_reader.notifications.send(payload);
                }
            }
            tracing::debug!("mcp ws reader exited");
        });

        Ok(Self { inner })
    }

    async fn send_text(&self, payload: String) -> Result<(), TakoError> {
        let mut guard = self.inner.sink.lock().await;
        let sink = guard
            .as_mut()
            .ok_or_else(|| TakoError::Transport("transport closed".into()))?;
        sink.send(Message::Text(payload.into()))
            .await
            .map_err(|e| TakoError::Transport(format!("ws send: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl McpTransport for WebSocketTransport {
    async fn request(&self, method: &str, params: Value) -> Result<Value, TakoError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let payload = request(id, method, params);

        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

        if let Err(e) = self.send_text(payload).await {
            self.inner.pending.lock().await.remove(&id);
            return Err(e);
        }

        match rx.await {
            Ok(r) => r,
            Err(_) => Err(TakoError::Transport("rpc channel closed".into())),
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), TakoError> {
        let payload = notification(method, params);
        self.send_text(payload).await
    }

    async fn notifications(&self) -> BoxStream<'static, Result<Value, TakoError>> {
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
        let mut guard = self.inner.sink.lock().await;
        if let Some(mut sink) = guard.take() {
            let _ = sink.send(Message::Close(None)).await;
            let _ = sink.close().await;
        }
        Ok(())
    }
}
