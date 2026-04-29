//! `tako-mcp` — MCP (Model Context Protocol) client transports and tool
//! registry.
//!
//! Phase 1 ships:
//!
//! - [`transport::stdio::StdioTransport`] — spawns a subprocess and
//!   exchanges newline-delimited JSON-RPC over its stdin/stdout.
//! - [`transport::streamable_http::StreamableHttpTransport`] — single-
//!   endpoint POST/GET. The SSE upgrade and `Mcp-Session-Id` lifecycle
//!   refinements arrive in Phase 2.
//! - [`registry::ToolRegistry`] — merges native [`tako_core::Tool`] impls
//!   with MCP-discovered tools.
//! - [`lifecycle::handshake`] — `initialize` → `initialized` lifecycle.

mod jsonrpc;
pub mod lifecycle;
pub mod registry;
pub mod transport;

pub use lifecycle::{ClientInfo, MCP_PROTOCOL_VERSION, handshake};
pub use registry::ToolRegistry;
#[cfg(feature = "ws")]
pub use transport::WebSocketTransport;
pub use transport::{StdioTransport, StreamableHttpBuilder, StreamableHttpTransport};
