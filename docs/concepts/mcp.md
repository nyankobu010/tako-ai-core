# MCP (Model Context Protocol)

`tako-mcp` wraps the [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
(`rmcp`) to expose two transports as `tako._native` types:

- **`Stdio`** — spawns a subprocess speaking JSON-RPC over its stdin/stdout.
- **`StreamableHttp`** — talks to an MCP server over HTTP+SSE.

WebSocket and gRPC transports are queued for Phase 4.

## Discover and use tools

```python
import tako

mcp_servers = [
    tako.mcp.Stdio(
        command="npx",
        args=["-y", "@modelcontextprotocol/server-everything"],
    ),
    tako.mcp.Http(url="https://mcp.example/v1"),
]

agent = tako.SingleAgent(
    provider=tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."),
    mcp_servers=mcp_servers,
)
# At construction, tako runs the MCP lifecycle handshake against each
# server, calls `tools/list`, and merges the schemas into the agent's
# tool registry.
result = await agent.run("Use a tool to find the weather in Tokyo.")
```

## How tool dispatch works

1. The provider returns a `ContentPart::ToolCall { id, name, args }`.
2. The orchestrator looks `name` up in its `ToolRegistry`. The registry
   knows whether the tool is local-Rust or comes from an MCP server.
3. For an MCP-backed tool, the orchestrator calls
   `transport.call_tool(name, args)`; the transport dispatches to the
   right server, parses the JSON-RPC response, and returns it as a
   `ContentPart::ToolResult`.
4. The result feeds back into the next provider call.

## Limitations

- **No streaming tool results yet.** MCP supports streaming; tako today
  only consumes complete results.
- **No tool sampling.** MCP defines `sampling/createMessage` for
  servers that want to invoke their *own* models on the host's behalf.
  Phase 3 will add this.
