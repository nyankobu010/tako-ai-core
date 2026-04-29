"""Phase 7.B — `tako.SelfCaller.stream(...)` Python-facade smoke tests.

Verifies that the new async iteration surface yields the right
sequence of `OrchEvent`s through a recursion cycle.
"""

from __future__ import annotations

from typing import Any

import tako


async def test_self_caller_stream_passes_through_when_confident() -> None:
    """Guard returns 1.0 → exactly one `final` event, no recursion."""

    async def chat(_request: dict[str, Any]) -> str:
        return "the answer is 42"

    provider = tako.providers.PythonProvider("py:p", chat=chat)
    agent = tako.SingleAgent(provider=provider)
    guard = tako.guards.RuleBased(min_chars=1)
    sc = tako.SelfCaller(agent, guard, max_depth=3, min_confidence=0.5)

    finals = 0
    last_text: str | None = None
    saw_text = False
    async for ev in await sc.stream("hi"):
        if ev.kind == "assistant_text":
            assert ev.delta is not None
            saw_text = True
        elif ev.kind == "final":
            finals += 1
            last_text = ev.text

    assert finals == 1
    assert last_text == "the answer is 42"
    assert saw_text


async def test_self_caller_stream_recurses_until_max_depth() -> None:
    """Constantly low-confidence guard → emits exactly one outer
    `final` after `max_depth + 1` inner attempts. The forwarded text
    is the last attempt's output."""

    counter = {"n": 0}

    async def chat(_request: dict[str, Any]) -> str:
        counter["n"] += 1
        return f"attempt-{counter['n']}"

    provider = tako.providers.PythonProvider("py:p", chat=chat)
    agent = tako.SingleAgent(provider=provider)
    # Require >= 100 chars so each "attempt-N" is below threshold.
    guard = tako.guards.RuleBased(min_chars=100)
    sc = tako.SelfCaller(agent, guard, max_depth=2, min_confidence=0.99)

    finals = 0
    step_starts = 0
    last_text: str | None = None
    async for ev in await sc.stream("solve"):
        if ev.kind == "step_start":
            step_starts += 1
        elif ev.kind == "final":
            finals += 1
            last_text = ev.text

    assert finals == 1, "outer stream must yield exactly one final"
    assert counter["n"] == 3, "max_depth=2 means inner runs 3 times"
    assert last_text == "attempt-3"
    # Each inner SingleAgent stream emits one StepStart per inner step;
    # we ran 3 inner orchestrator invocations so the outer stream
    # forwards 3 StepStarts.
    assert step_starts == 3
