"""MCP transport wrappers (Phase 1.5).

The Rust crate `tako-mcp` ships StdioTransport and StreamableHttpTransport
today; the Python bindings for them land in Phase 1.5. The placeholder
classes below preserve the eventual API so user code written against
``tako.mcp.Stdio(...)`` will keep working when the bindings arrive.
"""

from __future__ import annotations


class _Placeholder:
    def __init__(self, *args: object, **kwargs: object) -> None:
        msg = (
            "tako.mcp transports are exposed via Rust today; the Python "
            "bindings arrive in Phase 1.5. Use the Rust API or wait for the "
            "next minor release."
        )
        raise NotImplementedError(msg)


class Stdio(_Placeholder):
    """MCP stdio transport. Spawns a subprocess and exchanges newline-delimited JSON-RPC."""


class Http(_Placeholder):
    """MCP Streamable HTTP transport (single endpoint POST/GET)."""


__all__ = ["Stdio", "Http"]
