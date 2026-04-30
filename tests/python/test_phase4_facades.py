"""Smoke tests for the Phase-4 PyO3 + facade additions (Phase 4.G).

Each block is gated on whether the wheel was built with the matching
Cargo feature: `ws`, `grpc`, `sigstore`, or `redis`. When the underlying
class isn't present (a feature-stripped build), the test
auto-skips so a default `pytest -q` invocation stays green.
"""

from __future__ import annotations

import json
import os

import pytest
import tako
from tako import _native


def _has(name: str) -> bool:
    return hasattr(_native, name)


# ---------- WebSocket transport (gated by `ws`) -----------------------------


@pytest.mark.skipif(not _has("WebSocket"), reason="wheel built without `ws` feature")
def test_websocket_class_is_exposed() -> None:
    cls = tako.mcp.WebSocket
    assert cls is not None


@pytest.mark.skipif(not _has("WebSocket"), reason="wheel built without `ws` feature")
def test_websocket_bad_url_raises() -> None:
    # Connecting to a port nothing listens on must surface as an error,
    # not a hang. Bind a TCP listener, drop it to free the port, and dial
    # the freed address.
    import socket

    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    addr = s.getsockname()
    s.close()
    with pytest.raises(ValueError):
        tako.mcp.WebSocket(f"ws://127.0.0.1:{addr[1]}")


# ---------- gRPC transport (gated by `grpc`) --------------------------------


@pytest.mark.skipif(not _has("Grpc"), reason="wheel built without `grpc` feature")
def test_grpc_class_is_exposed() -> None:
    assert tako.mcp.Grpc is not None


@pytest.mark.skipif(not _has("Grpc"), reason="wheel built without `grpc` feature")
def test_grpc_bad_endpoint_raises() -> None:
    import socket

    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    addr = s.getsockname()
    s.close()
    with pytest.raises(ValueError):
        tako.mcp.Grpc(f"http://127.0.0.1:{addr[1]}")


# ---------- Sigstore CatalogueVerifier (gated by `sigstore`) ----------------


@pytest.mark.skipif(not _has("CatalogueVerifier"), reason="wheel built without `sigstore` feature")
def test_catalogue_round_trip_via_cryptography() -> None:
    """End-to-end: generate an ECDSA-P256 keypair via `cryptography`,
    sign a sample manifest, and round-trip it through
    ``tako.sigstore.CatalogueVerifier``."""
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec

    sk = ec.generate_private_key(ec.SECP256R1())
    pem = sk.public_key().public_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PublicFormat.SubjectPublicKeyInfo,
    )
    manifest = json.dumps(
        {
            "server": "https://mcp.example.com",
            "tools": [
                {
                    "name": "weather.lookup",
                    "description": "Look up the weather for a city.",
                    "input_schema": {
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                        "required": ["city"],
                    },
                }
            ],
        }
    ).encode()
    signature = sk.sign(manifest, ec.ECDSA(hashes.SHA256()))

    verifier = tako.sigstore.CatalogueVerifier(pem)
    catalogue = verifier.verify(manifest, signature)
    assert catalogue.server == "https://mcp.example.com"
    assert len(catalogue.tools) == 1
    assert catalogue.tools[0].name == "weather.lookup"


@pytest.mark.skipif(not _has("CatalogueVerifier"), reason="wheel built without `sigstore` feature")
def test_catalogue_rejects_tampered_manifest() -> None:
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec

    sk = ec.generate_private_key(ec.SECP256R1())
    pem = sk.public_key().public_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PublicFormat.SubjectPublicKeyInfo,
    )
    manifest = b'{"tools":[]}'
    signature = sk.sign(manifest, ec.ECDSA(hashes.SHA256()))

    tampered = b'{"tools":[{"name":"evil"}]}'  # different bytes, same shape
    verifier = tako.sigstore.CatalogueVerifier(pem)
    with pytest.raises(ValueError, match="signature invalid"):
        verifier.verify(tampered, signature)


# ---------- Redis BudgetBackend (gated by `redis`) --------------------------


def _redis_url() -> str | None:
    """Return REDIS_URL or None (so tests auto-skip without a server)."""
    return os.environ.get("REDIS_URL")


@pytest.mark.skipif(not _has("RedisBudgetBackend"), reason="wheel built without `redis` feature")
@pytest.mark.skipif(_redis_url() is None, reason="REDIS_URL unset")
async def test_redis_backend_round_trip() -> None:
    backend = tako.budget.RedisBackend(
        _redis_url(),
        key_prefix=f"tako:test:py:{os.getpid()}",
    )
    await backend.record("acme", 0.25, 100)
    usage = await backend.current_usage("acme")
    assert abs(usage.usd_today - 0.25) < 1e-9
    assert usage.tokens_today == 100


@pytest.mark.skipif(not _has("RedisBudgetBackend"), reason="wheel built without `redis` feature")
@pytest.mark.skipif(_redis_url() is None, reason="REDIS_URL unset")
async def test_redis_backend_zero_for_unknown_tenant() -> None:
    backend = tako.budget.RedisBackend(
        _redis_url(),
        key_prefix=f"tako:test:py:{os.getpid()}:zero",
    )
    usage = await backend.current_usage("never-recorded")
    assert usage.usd_today == 0.0
    assert usage.tokens_today == 0
