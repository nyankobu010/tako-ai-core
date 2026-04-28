//! Stdio JSON-RPC transport: spawns a subprocess and exchanges
//! newline-delimited JSON-RPC over its stdin/stdout.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::Value;
use tako_core::{McpTransport, TakoError};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, broadcast, oneshot};

use crate::jsonrpc::{Response, notification, request};

const NOTIFICATION_BUFFER: usize = 64;

#[derive(Debug)]
struct Inner {
    next_id: AtomicU64,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, TakoError>>>>,
    notifications: broadcast::Sender<Value>,
    child: Mutex<Option<Child>>,
}

/// MCP stdio transport.
#[derive(Clone, Debug)]
pub struct StdioTransport {
    inner: Arc<Inner>,
}

impl StdioTransport {
    /// Spawn `command` with `args` and start the read loop. The process is
    /// expected to speak newline-delimited JSON-RPC 2.0 on stdout.
    pub async fn spawn(command: &str, args: &[String]) -> Result<Self, TakoError> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| TakoError::Transport(format!("spawn `{command}`: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TakoError::Transport("child stdin missing".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TakoError::Transport("child stdout missing".into()))?;

        let (tx, _rx) = broadcast::channel(NOTIFICATION_BUFFER);
        let inner = Arc::new(Inner {
            next_id: AtomicU64::new(1),
            stdin: Mutex::new(Some(stdin)),
            pending: Mutex::new(HashMap::new()),
            notifications: tx.clone(),
            child: Mutex::new(Some(child)),
        });

        // Reader task: parse one JSON object per line and dispatch.
        let inner_reader = Arc::clone(&inner);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let parsed: Response = match serde_json::from_str(&line) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, line = %line, "mcp: invalid JSON-RPC frame");
                        continue;
                    }
                };
                if let Some(id) = parsed.id {
                    if let Some(tx) = inner_reader.pending.lock().await.remove(&id) {
                        let result = if let Some(err) = parsed.error {
                            Err(TakoError::Transport(format!("rpc error {}: {}", err.code, err.message)))
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
            tracing::debug!("mcp stdio reader exited");
        });

        Ok(Self { inner })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, method: &str, params: Value) -> Result<Value, TakoError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let payload = format!("{}\n", request(id, method, params));

        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

        {
            let mut guard = self.inner.stdin.lock().await;
            let stdin = guard
                .as_mut()
                .ok_or_else(|| TakoError::Transport("transport closed".into()))?;
            stdin
                .write_all(payload.as_bytes())
                .await
                .map_err(|e| TakoError::Transport(format!("write: {e}")))?;
            stdin
                .flush()
                .await
                .map_err(|e| TakoError::Transport(format!("flush: {e}")))?;
        }

        match rx.await {
            Ok(r) => r,
            Err(_) => Err(TakoError::Transport("rpc channel closed".into())),
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), TakoError> {
        let payload = format!("{}\n", notification(method, params));
        let mut guard = self.inner.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| TakoError::Transport("transport closed".into()))?;
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| TakoError::Transport(format!("write: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| TakoError::Transport(format!("flush: {e}")))?;
        Ok(())
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
        // Drop stdin (signalling EOF to the child) and reap.
        {
            let mut g = self.inner.stdin.lock().await;
            *g = None;
        }
        let mut child_guard = self.inner.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        Ok(())
    }
}
