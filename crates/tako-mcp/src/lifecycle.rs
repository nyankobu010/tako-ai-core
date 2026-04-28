//! MCP 2025-06-18 lifecycle: `initialize` handshake, capability negotiation,
//! `initialized` notification.

use std::sync::Arc;

use serde_json::{Value, json};
use tako_core::{McpTransport, TakoError};

pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Capabilities the `tako` client advertises in `initialize`.
#[derive(Clone, Debug, Default)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

impl ClientInfo {
    pub fn tako() -> Self {
        Self {
            name: "tako".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

/// Run the `initialize` → `initialized` handshake. Returns the server's
/// `serverInfo + capabilities` JSON.
pub async fn handshake(
    transport: Arc<dyn McpTransport>,
    client: ClientInfo,
) -> Result<Value, TakoError> {
    let init_params = json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {
            "roots": { "listChanged": false },
            "sampling": {},
        },
        "clientInfo": {
            "name": client.name,
            "version": client.version,
        }
    });
    let server = transport.request("initialize", init_params).await?;
    transport
        .notify("notifications/initialized", Value::Null)
        .await?;
    Ok(server)
}
