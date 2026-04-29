//! MCP transport implementations.

#[cfg(feature = "grpc")]
pub mod grpc;
pub mod stdio;
pub mod streamable_http;
#[cfg(feature = "ws")]
pub mod websocket;

#[cfg(feature = "grpc")]
pub use grpc::GrpcTransport;
pub use stdio::StdioTransport;
pub use streamable_http::{StreamableHttpBuilder, StreamableHttpTransport};
#[cfg(feature = "ws")]
pub use websocket::WebSocketTransport;
