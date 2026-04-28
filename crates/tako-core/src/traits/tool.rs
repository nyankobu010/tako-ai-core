//! `Tool` — a function the model can call.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::TakoError;
use crate::types::{Principal, ToolSchema};

/// A function the model can call. Implementations come from native Rust
/// code, MCP-discovered tools, or Python code via `tako-py`.
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    /// JSON-Schema describing the tool's input.
    fn schema(&self) -> &ToolSchema;

    /// Optional Sigstore signature material. `None` means unsigned. Phase 4
    /// adds verification of this against a configured trust root.
    fn signature(&self) -> Option<&[u8]> {
        None
    }

    /// Invoke the tool. Implementations must validate `args` against
    /// `schema()` if they require structural guarantees; the caller will
    /// surface validation errors as `TakoError::Tool`.
    async fn invoke(&self, principal: &Principal, args: Value) -> Result<Value, TakoError>;
}
