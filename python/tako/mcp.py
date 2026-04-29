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


class WebSocket(_TransportBase):
    """MCP WebSocket transport. Bidirectional JSON-RPC over a single
    ``ws://`` or ``wss://`` connection. Available when the wheel was built
    with the ``ws`` Cargo feature (raises ``AttributeError`` otherwise)."""

    def __init__(self, url: str) -> None:
        self._native = _native.WebSocket(url)


class Grpc(_TransportBase):
    """MCP gRPC transport. JSON-RPC frames carried over a single bidi
    streaming RPC. Endpoint is ``http://host:port`` (plaintext) or
    ``https://host:port`` (rustls + webpki-roots). Available when the
    wheel was built with the ``grpc`` Cargo feature."""

    def __init__(self, endpoint: str) -> None:
        self._native = _native.Grpc(endpoint)


__all__ = ["Grpc", "Http", "Stdio", "WebSocket"]
