//! `McpTransport` — abstraction over an MCP transport (stdio / Streamable
//! HTTP / WebSocket / gRPC). Implementations live in `tako-mcp`.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::Value;

use crate::error::TakoError;

/// One MCP transport binding to a single server. Lifecycle management
/// (`initialize` handshake, capability negotiation) is the responsibility of
/// the implementation; consumers see only the JSON-RPC surface here.
#[async_trait]
pub trait McpTransport: Send + Sync + 'static {
    /// JSON-RPC request/response.
    async fn request(&self, method: &str, params: Value) -> Result<Value, TakoError>;

    /// JSON-RPC notification (fire-and-forget).
    async fn notify(&self, method: &str, params: Value) -> Result<(), TakoError>;

    /// Server → client notification stream
    /// (e.g. `notifications/tools/list_changed`, `notifications/progress`).
    async fn notifications(&self) -> BoxStream<'static, Result<Value, TakoError>>;

    /// Close the transport and release any resources (e.g. child processes,
    /// HTTP sessions). Idempotent.
    async fn close(&self) -> Result<(), TakoError>;
}
