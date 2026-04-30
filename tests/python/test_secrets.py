"""Smoke tests for cloud secret resolver Python bindings.

Wire-format correctness is covered by the Rust crate's wiremock tests in
``crates/tako-governance/tests/secrets.rs``; this file exercises the
Python facade construction + the async ``resolve()`` happy path against
an in-process aiohttp server (Vault) so we know the binding actually
hands data through.

For Azure KV and GCP SM the Python binding shape is identical; we trust
the Rust-side tests there. AWS SM is exercised at the constructor level
only — full LocalStack-backed coverage is out of scope.
"""

from __future__ import annotations

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import pytest

import tako


def _serve_once(
    routes: dict[str, dict[str, Any]],
) -> tuple[str, ThreadingHTTPServer, threading.Thread]:
    """Spin up a single-purpose HTTP server returning canned JSON for the
    given path -> body map. Returns (base_url, server, thread)."""

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 — stdlib API
            for path, payload in routes.items():
                if self.path.startswith(path):
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    body = json.dumps(payload).encode()
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
            self.send_response(404)
            self.end_headers()

        def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
            return  # silence default access log

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    return f"http://{host}:{port}", server, thread


async def test_vault_resolver_reads_secret() -> None:
    base, server, thread = _serve_once(
        {"/v1/secret/data/myapp": {"data": {"data": {"api_key": "sk-test"}}}}
    )
    try:
        resolver = tako.secrets.VaultResolver(base, "vault-token")
        value = await resolver.resolve("secret/data/myapp#api_key")
        assert value == "sk-test"
    finally:
        server.shutdown()
        thread.join(timeout=2)


async def test_vault_resolver_missing_subkey() -> None:
    base, server, thread = _serve_once(
        {"/v1/secret/data/myapp": {"data": {"data": {"foo": "bar"}}}}
    )
    try:
        resolver = tako.secrets.VaultResolver(base, "vault-token")
        with pytest.raises(ValueError):
            await resolver.resolve("secret/data/myapp#missing")
    finally:
        server.shutdown()
        thread.join(timeout=2)


def test_azure_key_vault_constructs() -> None:
    r = tako.secrets.AzureKeyVaultResolver(
        "https://my-vault.vault.azure.net",
        "azure-token",
    )
    assert r is not None


def test_azure_key_vault_with_api_version() -> None:
    r = tako.secrets.AzureKeyVaultResolver(
        "https://my-vault.vault.azure.net",
        "azure-token",
        api_version="7.5",
    )
    assert r is not None


def test_gcp_secret_manager_constructs() -> None:
    r = tako.secrets.GcpSecretManagerResolver(
        "my-proj",
        "gcp-token",
        endpoint_url="https://example.test",
    )
    assert r is not None


def test_aws_secrets_manager_constructs_without_credentials() -> None:
    r = tako.secrets.AwsSecretsManagerResolver(
        region="us-west-2",
        profile_name="nonexistent",
        endpoint_url="http://127.0.0.1:1",
    )
    assert r is not None


async def test_resolver_classes_exposed_on_package() -> None:
    # Ensure the public facade matches the documented API surface.
    assert hasattr(tako.secrets, "VaultResolver")
    assert hasattr(tako.secrets, "AzureKeyVaultResolver")
    assert hasattr(tako.secrets, "GcpSecretManagerResolver")
    assert hasattr(tako.secrets, "AwsSecretsManagerResolver")
