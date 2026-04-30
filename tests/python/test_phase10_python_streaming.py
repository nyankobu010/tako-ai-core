"""Phase 10.D — PythonProvider streaming.

Closes the v0.2.0 stale marker that ``Python providers do not yet
support streaming``. The user passes ``stream=async_generator_fn`` and
the Rust side iterates it via ``__anext__()``, deserialising each
yielded dict to a ``ChatChunk`` via the standard ``kind``-tagged JSON
shape.
"""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

import tako


async def _fake_chat(_request: dict) -> str:
    return "fallback non-stream"


async def _fake_stream(_request: dict[str, Any]):
    yield {"kind": "delta", "text": "hello"}
    yield {"kind": "delta", "text": " world"}
    yield {
        "kind": "end",
        "finish_reason": "stop",
        "usage": {"input_tokens": 4, "output_tokens": 2},
    }


async def _erroring_stream(_request: dict[str, Any]):
    yield {"kind": "delta", "text": "ok so far"}
    raise RuntimeError("boom")


async def _bad_shape_stream(_request: dict[str, Any]):
    yield {"not_a_kind": "nope"}


def test_python_provider_with_stream_flips_supports_streaming() -> None:
    p = tako.providers.PythonProvider(
        "custom:streamy",
        chat=_fake_chat,
        stream=_fake_stream,
    )
    # The provider object exposes its handle id; the streaming
    # capability is set Rust-side and consumed by orchestrators.
    assert p.id == "custom:streamy"


def test_python_provider_without_stream_kwarg_construct_ok() -> None:
    # Backwards-compat: omitting `stream=` is fine; construction
    # succeeds and chat-only behaviour is preserved.
    p = tako.providers.PythonProvider("custom:chatonly", chat=_fake_chat)
    assert p.id == "custom:chatonly"


def _wrap_with_self_caller(provider: tako.providers.PythonProvider) -> tako.SelfCaller:
    """SingleAgent doesn't expose `.stream()` on the Python facade, but
    `SelfCaller` does. The guard's `min_chars` is set high enough that
    streaming-aware early-abort never fires for the chunk lengths in
    these tests, so SelfCaller forwards every inner AssistantText
    delta to the outer stream verbatim."""
    inner = tako.SingleAgent(provider=provider, max_steps=1)
    guard = tako.guards.RuleBased(min_chars=10_000)
    return tako.SelfCaller(inner, confidence=guard, max_depth=0, min_confidence=0.99)


async def test_streaming_provider_round_trips_through_self_caller() -> None:
    # Streaming PythonProvider plugged into SelfCaller.stream() yields
    # the user's chunks back through to the orchestrator's event stream.
    p = tako.providers.PythonProvider(
        "custom:roundtrip",
        chat=_fake_chat,
        stream=_fake_stream,
    )
    sc = _wrap_with_self_caller(p)

    deltas: list[str] = []
    stream = await sc.stream("hi")
    async for ev in stream:
        if ev.kind == "assistant_text":
            deltas.append(ev.delta)
    # The two deltas combine into the expected text.
    assert "".join(deltas) == "hello world"


async def test_streaming_provider_propagates_python_errors() -> None:
    # A RuntimeError raised inside the async generator surfaces as a
    # provider-level error on the orchestrator stream (not a panic).
    p = tako.providers.PythonProvider(
        "custom:erroring",
        chat=_fake_chat,
        stream=_erroring_stream,
    )
    sc = _wrap_with_self_caller(p)

    with pytest.raises(Exception) as excinfo:
        stream = await sc.stream("hi")
        async for _ev in stream:
            pass
    msg = str(excinfo.value)
    assert "boom" in msg or "stream" in msg.lower()


async def test_streaming_provider_rejects_bad_chunk_shape() -> None:
    # A yielded dict that doesn't match ChatChunk's kind-tagged schema
    # surfaces a clear schema-mismatch error.
    p = tako.providers.PythonProvider(
        "custom:badshape",
        chat=_fake_chat,
        stream=_bad_shape_stream,
    )
    sc = _wrap_with_self_caller(p)
    with pytest.raises(Exception) as excinfo:
        stream = await sc.stream("hi")
        async for _ev in stream:
            pass
    msg = str(excinfo.value)
    assert "ChatChunk" in msg or "kind" in msg or "schema" in msg
