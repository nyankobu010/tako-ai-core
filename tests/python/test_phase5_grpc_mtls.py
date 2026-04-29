"""Smoke tests for the Phase 5 ``tako.mcp.Grpc`` mTLS kwargs.

End-to-end mTLS coverage lives in the Rust integration tests
(``crates/tako-mcp/tests/grpc.rs::mtls``); from Python we exercise the
facade-level validation that checks how the kwargs combine.
"""

from __future__ import annotations

import pytest
import tako
from tako import _native


def _has(name: str) -> bool:
    return hasattr(_native, name)


@pytest.mark.skipif(not _has("Grpc"), reason="wheel built without `grpc` feature")
def test_inline_and_path_pem_are_mutually_exclusive() -> None:
    with pytest.raises(ValueError, match="ca_pem"):
        tako.mcp.Grpc(
            "http://127.0.0.1:1",
            ca_pem=b"-----BEGIN CERTIFICATE-----\n",
            ca_path="/dev/null",
        )


@pytest.mark.skipif(not _has("Grpc"), reason="wheel built without `grpc` feature")
def test_client_cert_without_ca_pem_raises() -> None:
    # The Rust layer treats `client_cert_pem` without `ca_pem` as a
    # configuration error and raises eagerly so the failure is
    # synchronous, not a mid-handshake surprise.
    with pytest.raises(ValueError, match="ca_pem is required"):
        tako.mcp.Grpc(
            "http://127.0.0.1:1",
            client_cert_pem=b"-----BEGIN CERTIFICATE-----\n",
            client_key_pem=b"-----BEGIN PRIVATE KEY-----\n",
        )


@pytest.mark.skipif(not _has("Grpc"), reason="wheel built without `grpc` feature")
def test_half_pair_client_identity_raises() -> None:
    # Passing only `client_cert_pem` (no key) must surface an error
    # before any handshake attempt.
    with pytest.raises(ValueError, match="client_cert_pem and client_key_pem"):
        tako.mcp.Grpc(
            "https://127.0.0.1:1",
            ca_pem=b"-----BEGIN CERTIFICATE-----\nABC\n-----END CERTIFICATE-----\n",
            client_cert_pem=b"-----BEGIN CERTIFICATE-----\n",
        )
