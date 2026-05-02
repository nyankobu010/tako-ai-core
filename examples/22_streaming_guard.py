"""Phase 8.D: streaming-aware ConfidenceGuard early-abort.

`tako.guards.RuleBased` overrides the new
`ConfidenceGuard::evaluate_streaming` hook: it returns 1.0 the
moment the cumulative assistant text reaches `min_chars`, so the
`SelfCaller` stream short-circuits the inner generation and emits
a terminal `final` carrying the accumulated partial.

Guards that don't override the streaming hook (e.g.
`tako.guards.LlmJudge`) keep the previous post-buffer behaviour —
the default is `Ok(None)`, which means "keep streaming and
evaluate the buffered final text".
"""

from __future__ import annotations

import asyncio
from typing import Any

import tako


async def main() -> None:
    # Custom Python provider that yields a long deterministic answer.
    async def chat(_request: dict[str, Any]) -> str:
        return "0123456789ABCDEFGHIJ"  # 20 characters

    provider = tako.providers.PythonProvider("py:p", chat=chat)
    inner = tako.SingleAgent(provider=provider, max_steps=1)
    sc = tako.SelfCaller(
        inner,
        tako.guards.RuleBased(min_chars=10),
        max_depth=3,
        min_confidence=0.5,
    )

    async for ev in await sc.stream("anything"):
        if ev.kind == "assistant_text":
            print(f"delta: {ev.delta!r}")
        elif ev.kind == "recursion":
            print(f"[recursion] depth={ev.depth} confidence={ev.confidence:.2f}")
        elif ev.kind == "final":
            print(f"[final] {ev.text!r}")


if __name__ == "__main__":
    asyncio.run(main())
