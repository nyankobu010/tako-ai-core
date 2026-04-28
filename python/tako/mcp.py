"""MCP transport wrappers."""

from __future__ import annotations

from typing import Any

from tako import _native


class _TransportBase:
    _native: Any

    def __repr__(self) -> str:
        return repr(self._native)


class Stdio(_TransportBase):
    """MCP stdio transport. Spawns a subprocess and exchanges newline-delimited
    JSON-RPC. The ``initialize`` → ``initialized`` handshake runs at
    construction time and blocks until it completes."""

    def __init__(self, command: str, args: list[str] | None = None) -> None:
        self._native = _native.Stdio(command, args)


class Http(_TransportBase):
    """MCP Streamable HTTP transport. Single-endpoint POST/GET; SSE upgrade
    arrives in Phase 2."""

    def __init__(
        self,
        url: str,
        *,
        headers: list[tuple[str, str]] | None = None,
        timeout_secs: int | None = None,
    ) -> None:
        self._native = _native.StreamableHttp(url, headers=headers, timeout_secs=timeout_secs)


__all__ = ["Http", "Stdio"]
