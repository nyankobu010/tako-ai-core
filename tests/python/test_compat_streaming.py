"""End-to-end test of tako.compat streaming.

Phase 2.5 wires SSE streaming into POST /v1/chat/completions when
``stream=True``. This test boots the server with a Fake provider and
reads the SSE stream raw via ``requests`` — the same wire format the
official openai Python SDK consumes (it just calls ``requests`` under
the hood). If the openai SDK is installed in the env, we additionally
assert it parses our chunks without error.
"""

from __future__ import annotations

import json
from collections.abc import Iterator

import pytest
import requests

import tako


@pytest.fixture
def compat_server() -> Iterator[str]:
    fake = tako.providers.Fake(canned_text="streamed-hello")
    agent = tako.SingleAgent(provider=fake)
    url = tako.compat.serve_openai(
        agent,
        host="127.0.0.1",
        port=0,
        tokens={"test-token": ("acme", "alice")},
        models=["tako-default"],
    )
    try:
        yield url
    finally:
        tako.compat.shutdown_openai()


def test_stream_emits_chunks_and_done(compat_server: str) -> None:
    r = requests.post(
        f"{compat_server}/v1/chat/completions",
        headers={"Authorization": "Bearer test-token"},
        json={
            "model": "tako-default",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": True,
        },
        timeout=5,
        stream=True,
    )
    assert r.status_code == 200
    assert r.headers["Content-Type"].startswith("text/event-stream")

    chunks: list[dict] = []
    saw_done = False
    for line in r.iter_lines(decode_unicode=True):
        if not line or not line.startswith("data: "):
            continue
        payload = line[len("data: ") :]
        if payload == "[DONE]":
            saw_done = True
            break
        chunks.append(json.loads(payload))

    assert saw_done, "stream did not terminate with `data: [DONE]`"
    assert chunks, "stream emitted no chunks before [DONE]"
    # Every chunk has the OpenAI-required shape.
    for c in chunks:
        assert c["object"] == "chat.completion.chunk"
        assert c["model"] == "tako-default"
        assert isinstance(c["choices"], list)

    # At least one chunk has content; the final chunk has finish_reason.
    has_content = any(c["choices"][0]["delta"].get("content") for c in chunks)
    assert has_content, "no chunk carried content delta"
    final_reasons = [c["choices"][0].get("finish_reason") for c in chunks]
    assert "stop" in final_reasons, f"no finish_reason=stop: {final_reasons}"

    # Reassembled text equals the Fake's canned response.
    text = "".join(c["choices"][0]["delta"].get("content", "") for c in chunks)
    assert text == "streamed-hello"


def test_openai_sdk_parses_stream(compat_server: str) -> None:
    pytest.importorskip("openai")
    from openai import OpenAI  # type: ignore[import-not-found]

    client = OpenAI(base_url=f"{compat_server}/v1", api_key="test-token")
    stream = client.chat.completions.create(
        model="tako-default",
        messages=[{"role": "user", "content": "hi"}],
        stream=True,
    )
    parts: list[str] = []
    finish_reasons: list[str] = []
    for chunk in stream:
        if chunk.choices and chunk.choices[0].delta and chunk.choices[0].delta.content:
            parts.append(chunk.choices[0].delta.content)
        if chunk.choices and chunk.choices[0].finish_reason:
            finish_reasons.append(chunk.choices[0].finish_reason)
    assert "".join(parts) == "streamed-hello"
    assert "stop" in finish_reasons
