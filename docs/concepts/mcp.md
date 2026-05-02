# MCP (Model Context Protocol)

`tako-mcp` wraps the [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
(`rmcp`) to expose four transports as `tako._native` types:

- **`Stdio`** — spawns a subprocess speaking JSON-RPC over its stdin/stdout.
- **`StreamableHttp`** — talks to an MCP server over HTTP, including the
  `notifications()` SSE channel and the `Mcp-Session-Id` lifecycle
  header captured from the initial POST.
- **`WebSocket`** — bidirectional, low-latency transport for MCP servers
  speaking the JSON-RPC-over-WebSocket variant.
- **`Grpc`** — gRPC transport with full mTLS support for production
  deployments where mutual identity is required.

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

## Notifications

`StreamableHttp.notifications()` opens a long-lived `GET {url}` over
`text/event-stream`, parses each `data:` line as JSON-RPC, and
broadcasts method-bearing frames to subscribers via
`tokio::sync::broadcast`. The latched `Mcp-Session-Id` header from the
initial POST is attached to the GET; `close()` shuts the channel down
via `tokio::sync::Notify`.

## Limitations

- **No streaming tool results yet.** MCP supports streaming; tako today
  only consumes complete results.
- **No tool sampling.** MCP defines `sampling/createMessage` for
  servers that want to invoke their *own* models on the host's behalf.
  On the roadmap; not yet shipped.
