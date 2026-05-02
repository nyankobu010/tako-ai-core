//! Tool registry — merges native Rust [`Tool`] impls with MCP-discovered
//! tools. Native tools are looked up by name; MCP tools are dispatched
//! through the originating transport.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tako_core::{McpTransport, Principal, TakoError, Tool, ToolSchema};
use tokio::sync::RwLock;

/// Source of a tool: a native `Tool` impl, or an MCP transport (the tool's
/// schema was discovered via `tools/list` and invocation routes through
/// `tools/call`).
enum ToolEntry {
    Native(Arc<dyn Tool>),
    Mcp {
        schema: ToolSchema,
        transport: Arc<dyn McpTransport>,
    },
}

#[derive(Default)]
pub struct ToolRegistry {
    inner: RwLock<HashMap<String, ToolEntry>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry").finish_non_exhaustive()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a native Rust tool.
    pub async fn register_native(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.inner
            .write()
            .await
            .insert(name, ToolEntry::Native(tool));
    }

    /// Register tools discovered from an MCP server's `tools/list`. The
    /// transport is held until the registry is dropped or explicitly cleared.
    pub async fn register_mcp(&self, transport: Arc<dyn McpTransport>, schemas: Vec<ToolSchema>) {
        let mut g = self.inner.write().await;
        for schema in schemas {
            g.insert(
                schema.name.clone(),
                ToolEntry::Mcp {
                    schema,
                    transport: Arc::clone(&transport),
                },
            );
        }
    }

    /// Discover tools from an MCP server via `tools/list` and register them.
    pub async fn discover(&self, transport: Arc<dyn McpTransport>) -> Result<usize, TakoError> {
        let result = transport.request("tools/list", Value::Null).await?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut schemas = Vec::with_capacity(tools.len());
        for t in tools {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TakoError::Invalid("MCP tools/list entry missing `name`".into()))?
                .to_string();
            let description = t
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            schemas.push(ToolSchema {
                name,
                description,
                input_schema,
                annotations: None,
            });
        }
        let n = schemas.len();
        self.register_mcp(transport, schemas).await;
        Ok(n)
    }

    /// Schemas of all currently registered tools (sorted by name).
    pub async fn schemas(&self) -> Vec<ToolSchema> {
        let g = self.inner.read().await;
        let mut out: Vec<ToolSchema> = g
            .values()
            .map(|e| match e {
                ToolEntry::Native(t) => t.schema().clone(),
                ToolEntry::Mcp { schema, .. } => schema.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Invoke a tool by name. Errors with [`TakoError::NotFound`] if the
    /// name is unknown.
    pub async fn invoke(
        &self,
        principal: &Principal,
        name: &str,
        args: Value,
    ) -> Result<Value, TakoError> {
        let g = self.inner.read().await;
        match g.get(name) {
            None => Err(TakoError::NotFound(format!("tool `{name}`"))),
            Some(ToolEntry::Native(tool)) => tool.invoke(principal, args).await,
            Some(ToolEntry::Mcp { transport, .. }) => {
                let params = serde_json::json!({"name": name, "arguments": args});
                let resp = transport.request("tools/call", params).await?;
                Ok(resp)
            }
        }
    }
}
