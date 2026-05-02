"""Phase 10.D: Python custom provider streaming.

Pass ``stream=async_generator_fn`` to ``tako.providers.PythonProvider``
and the Rust side iterates the async generator via ``__anext__()``.
Each yielded dict deserialises to a ``ChatChunk`` via the standard
``kind``-tagged JSON shape:

    {"kind": "delta", "text": "..."}                    # partial text
    {"kind": "end", "finish_reason": "stop", "usage": ...}
    {"kind": "error", "message": "..."}                 # non-fatal

When ``stream=`` is supplied, the provider's ``supports_streaming``
capability flips to ``True`` so orchestrators that prefer streaming
(SelfCaller, AbMcts, Trinity) route through the streaming path
automatically.
"""

from __future__ import annotations

import asyncio
from typing import Any

import tako


async def chat_fallback(_request: dict[str, Any]) -> str:
    """Used when an orchestrator can't stream (e.g. SingleAgent.run)."""
    return "non-streaming fallback"


async def stream_chat(_request: dict[str, Any]):
    """Token-by-token output as an async generator.

    Each yielded dict matches the `ChatChunk` JSON schema. For longer
    outputs you can `await asyncio.sleep(0)` between chunks to let the
    runtime interleave, mirroring real network-backed streaming.
    """
    for token in ("hello ", "from ", "a Python ", "streaming ", "provider"):
        yield {"kind": "delta", "text": token}
        await asyncio.sleep(0)
    yield {
        "kind": "end",
        "finish_reason": "stop",
        "usage": {"input_tokens": 4, "output_tokens": 5},
    }


async def main() -> None:
    provider = tako.providers.PythonProvider(
        "custom:streamy",
        chat=chat_fallback,
        stream=stream_chat,
    )

    # `SingleAgent` doesn't expose `.stream()` on the Python facade in
    # v0.11.0 — wrap with `SelfCaller` which does. Set `min_chars` very
    # high so the streaming-aware early-abort never fires and the
    # underlying chunks all forward through.
    inner = tako.SingleAgent(provider=provider, max_steps=1)
    sc = tako.SelfCaller(
        inner,
        confidence=tako.guards.RuleBased(min_chars=10_000),
        max_depth=0,
        min_confidence=0.99,
    )

    print("streaming output:")
    async for ev in await sc.stream("hi"):
        if ev.kind == "assistant_text":
            print(f"  delta: {ev.delta!r}")
        elif ev.kind == "final":
            print(f"final: {ev.text!r}")


if __name__ == "__main__":
    asyncio.run(main())
