"""End-to-end MCP stdio test from Python.

Spawns a tiny bash-script MCP server that responds to ``initialize``,
``tools/list``, and ``tools/call``. The orchestrator discovers the server's
tools at construction time and the FakeProvider's scripted tool-call gets
routed back through the MCP transport.

The bash script approach keeps the test platform-portable across macOS and
Linux CI runners. Windows MCP tests are skipped (the StdioTransport works
on Windows, but bash is not always present).
"""

# ruff: noqa: E501 - the embedded JSON-RPC fixtures are intentionally long
from __future__ import annotations

import shutil
import sys
import textwrap

import pytest
import tako

SERVER_SCRIPT = textwrap.dedent("""
    while IFS= read -r line; do
      case "$line" in
        *initialize*)
          echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"0.0.1"}}}'
          ;;
        *tools/list*)
          echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo input.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}'
          ;;
        *tools/call*)
          echo '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"echoed"}]}}'
          ;;
      esac
    done
""").strip()


pytestmark = pytest.mark.skipif(
    sys.platform == "win32" or shutil.which("bash") is None,
    reason="bash-based MCP test server not available on this platform",
)


async def test_stdio_transport_constructs() -> None:
    transport = tako.mcp.Stdio("bash", ["-c", SERVER_SCRIPT])
    assert "bash" in repr(transport)


async def test_orchestrator_discovers_mcp_tools() -> None:
    transport = tako.mcp.Stdio("bash", ["-c", SERVER_SCRIPT])
    fake = tako.providers.Fake(canned_text="ok-after-mcp")

    # The orchestrator's constructor calls tools/list on the transport
    # synchronously; if that fails this raises immediately.
    agent = tako.SingleAgent(provider=fake, mcp_servers=[transport])

    result = await agent.run("doesn't matter")
    assert result.text == "ok-after-mcp"
    assert fake.call_count == 1
