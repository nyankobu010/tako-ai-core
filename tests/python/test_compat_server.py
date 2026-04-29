"""End-to-end test of tako.compat.serve_openai().

Boots the server on a free port, hits it with a real HTTP client, and
asserts the OpenAI-shaped response. We use `requests` rather than the
`openai` SDK to avoid the network dependency in CI; the wire-format
contract is what the test pins.
"""

from __future__ import annotations

from collections.abc import Iterator

import pytest
import requests
import tako


@pytest.fixture
def compat_server() -> Iterator[str]:
    """Boot the server on port 0, yield the URL, shut down after."""
    fake = tako.providers.Fake(canned_text="hello-from-compat")
    agent = tako.SingleAgent(provider=fake)
    url = tako.compat.serve_openai(
        agent,
        host="127.0.0.1",
        port=0,
        tokens={"test-token": ("acme", "alice")},
        models=["fake:m", "tako-default"],
    )
    try:
        yield url
    finally:
        tako.compat.shutdown_openai()


def test_health(compat_server: str) -> None:
    r = requests.get(f"{compat_server}/healthz", timeout=2)
    assert r.status_code == 200
    assert r.json() == {"status": "ok"}


def test_chat_completions(compat_server: str) -> None:
    r = requests.post(
        f"{compat_server}/v1/chat/completions",
        headers={"Authorization": "Bearer test-token"},
        json={
            "model": "tako-default",
            "messages": [{"role": "user", "content": "hi"}],
        },
        timeout=5,
    )
    assert r.status_code == 200
    data = r.json()
    assert data["object"] == "chat.completion"
    assert data["choices"][0]["message"]["content"] == "hello-from-compat"
    assert data["choices"][0]["finish_reason"] == "stop"
    # Usage shape mirrors the OpenAI SDK.
    assert "prompt_tokens" in data["usage"]
    assert "completion_tokens" in data["usage"]
    assert "total_tokens" in data["usage"]


def test_models(compat_server: str) -> None:
    r = requests.get(f"{compat_server}/v1/models", timeout=2)
    assert r.status_code == 200
    ids = [m["id"] for m in r.json()["data"]]
    assert "fake:m" in ids
    assert "tako-default" in ids


def test_missing_auth_returns_401(compat_server: str) -> None:
    r = requests.post(
        f"{compat_server}/v1/chat/completions",
        json={"model": "tako-default", "messages": [{"role": "user", "content": "x"}]},
        timeout=2,
    )
    assert r.status_code == 401
