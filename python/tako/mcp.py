"""MCP transport wrappers."""

from __future__ import annotations

from pathlib import Path
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


def _read_pem(inline: bytes | None, path: str | Path | None, label: str) -> bytes | None:
    """Resolve a PEM either from inline ``bytes`` or from a filesystem
    path. Exactly zero or one of the two must be provided.
    """
    if inline is not None and path is not None:
        raise ValueError(f"{label}: pass inline bytes or a path, not both")
    if inline is not None:
        return inline
    if path is not None:
        return Path(path).read_bytes()
    return None


class Grpc(_TransportBase):
    """MCP gRPC transport. JSON-RPC frames carried over a single bidi
    streaming RPC.

    Endpoint is ``http://host:port`` (plaintext) or ``https://host:port``
    for TLS. With no TLS kwargs the server cert is verified against
    ``webpki-roots``. Pass ``ca_pem`` (or ``ca_path``) to use a custom CA
    bundle, plus ``client_cert_pem`` / ``client_key_pem`` (or their
    ``_path`` siblings) together to enable mTLS. ``domain_name``
    overrides the SNI / cert-hostname check.

    Available when the wheel was built with the ``grpc`` Cargo feature.
    """

    def __init__(
        self,
        endpoint: str,
        *,
        ca_pem: bytes | None = None,
        ca_path: str | Path | None = None,
        client_cert_pem: bytes | None = None,
        client_cert_path: str | Path | None = None,
        client_key_pem: bytes | None = None,
        client_key_path: str | Path | None = None,
        domain_name: str | None = None,
    ) -> None:
        ca = _read_pem(ca_pem, ca_path, "ca_pem")
        cert = _read_pem(client_cert_pem, client_cert_path, "client_cert_pem")
        key = _read_pem(client_key_pem, client_key_path, "client_key_pem")
        self._native = _native.Grpc(
            endpoint,
            ca_pem=ca,
            client_cert_pem=cert,
            client_key_pem=key,
            domain_name=domain_name,
        )


__all__ = ["Grpc", "Http", "Stdio", "WebSocket"]
