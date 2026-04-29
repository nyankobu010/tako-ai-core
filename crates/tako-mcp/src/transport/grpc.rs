//! gRPC MCP transport.
//!
//! MCP itself is JSON-RPC 2.0; the spec doesn't standardise a gRPC
//! transport, so we hand-craft the smallest reasonable bridge: a
//! single bidirectional streaming RPC carrying opaque JSON frames
//! (`proto/mcp_bridge.proto`). Each `Frame { json }` is a complete
//! JSON-RPC message. The wire surface is intentionally schema-agnostic
//! so this transport rides whatever MCP version the underlying server
//! speaks without recompiling the `.proto`.
//!
//! Behaviour mirrors [`super::websocket::WebSocketTransport`]: a
//! background reader task demultiplexes incoming frames into
//! per-request `oneshot` channels (keyed by JSON-RPC `id`) and a
//! `tokio::sync::broadcast` channel for server-emitted notifications.
//!
//! Gated behind the `grpc` Cargo feature so `tonic` and the generated
//! protobuf code only land in the dep tree (and `protoc` is only
//! required at build time) when explicitly enabled.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde_json::Value;
use tako_core::{McpTransport, TakoError};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

use crate::jsonrpc::{Response, notification, request};

/// Generated protobuf types for `tako.mcp.bridge.v1`.
///
/// Public so the matching server stub (used in integration tests and by
/// downstream MCP server implementers) is reachable as
/// `tako_mcp::transport::grpc::proto::mcp_bridge_server::*`.
pub mod proto {
    tonic::include_proto!("tako.mcp.bridge.v1");
}

use proto::Frame;
use proto::mcp_bridge_client::McpBridgeClient;

const NOTIFICATION_BUFFER: usize = 64;
const OUTBOUND_BUFFER: usize = 64;

#[derive(Debug)]
struct Inner {
    next_id: AtomicU64,
    tx: Mutex<Option<mpsc::Sender<Frame>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, TakoError>>>>,
    notifications: broadcast::Sender<Value>,
}

/// MCP gRPC transport.
///
/// Connects to a server speaking `tako.mcp.bridge.v1.McpBridge` (see
/// `crates/tako-mcp/proto/mcp_bridge.proto`).
#[derive(Clone, Debug)]
pub struct GrpcTransport {
    inner: Arc<Inner>,
}

impl GrpcTransport {
    /// Connect to `endpoint` (e.g. `http://localhost:50051` or
    /// `https://example.com:443`) and start the demuxing reader task.
    ///
    /// `https://` URLs use rustls + webpki-roots for trust. mTLS and
    /// custom CAs are out of scope for this transport; wrap the
    /// underlying [`tonic::transport::Channel`] manually if you need
    /// them.
    pub async fn connect(endpoint: &str) -> Result<Self, TakoError> {
        let channel = tonic::transport::Channel::from_shared(endpoint.to_string())
            .map_err(|e| TakoError::Transport(format!("grpc endpoint: {e}")))?
            .connect()
            .await
            .map_err(|e| TakoError::Transport(format!("grpc connect: {e}")))?;
        let mut client = McpBridgeClient::new(channel);

        let (tx, rx) = mpsc::channel::<Frame>(OUTBOUND_BUFFER);
        let outbound = ReceiverStream::new(rx);

        let response = client
            .open(outbound)
            .await
            .map_err(|s| TakoError::Transport(format!("grpc open rpc: {s}")))?;
        let mut inbound = response.into_inner();

        let (notif_tx, _rx) = broadcast::channel(NOTIFICATION_BUFFER);
        let inner = Arc::new(Inner {
            next_id: AtomicU64::new(1),
            tx: Mutex::new(Some(tx)),
            pending: Mutex::new(HashMap::new()),
            notifications: notif_tx,
        });

        // Reader task: parse each inbound Frame as JSON-RPC and dispatch.
        let inner_reader = Arc::clone(&inner);
        tokio::spawn(async move {
            while let Some(item) = inbound.next().await {
                let frame = match item {
                    Ok(f) => f,
                    Err(s) => {
                        tracing::warn!(status = %s, "mcp grpc: inbound error");
                        break;
                    }
                };
                let text = match std::str::from_utf8(&frame.json) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "mcp grpc: non-UTF8 frame");
                        continue;
                    }
                };
                if text.trim().is_empty() {
                    continue;
                }
                let parsed: Response = match serde_json::from_str(text) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, frame = %text, "mcp grpc: invalid JSON-RPC frame");
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
            tracing::debug!("mcp grpc reader exited");
        });

        Ok(Self { inner })
    }

    async fn send_frame(&self, payload: String) -> Result<(), TakoError> {
        let guard = self.inner.tx.lock().await;
        let tx = guard
            .as_ref()
            .ok_or_else(|| TakoError::Transport("transport closed".into()))?;
        tx.send(Frame {
            json: payload.into_bytes(),
        })
        .await
        .map_err(|e| TakoError::Transport(format!("grpc send: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl McpTransport for GrpcTransport {
    async fn request(&self, method: &str, params: Value) -> Result<Value, TakoError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let payload = request(id, method, params);

        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

        if let Err(e) = self.send_frame(payload).await {
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
        self.send_frame(payload).await
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
        // Dropping the sender end-of-streams the outbound half; the
        // server's stream will end, which (for well-behaved servers)
        // closes the inbound half and unblocks the reader task.
        let _ = self.inner.tx.lock().await.take();
        Ok(())
    }
}
