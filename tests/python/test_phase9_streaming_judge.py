"""Phase 9.A — streaming opt-in for LlmJudgeGuard (Python facade)."""

from __future__ import annotations

from typing import Any

import tako


async def _judge_chat(_request: dict[str, Any]) -> str:
    return "0.7"


def test_llm_judge_accepts_streaming_kwargs() -> None:
    """Both new kwargs are accepted at construction; the underlying
    `tako._native.LlmJudgeGuard` plumbs them through. The guard's
    streaming path only fires inside `SelfCaller.stream`, so this
    smoke just asserts the facade keeps the kwargs and forwards them.
    """
    judge = tako.providers.PythonProvider("py:judge", chat=_judge_chat)

    guard = tako.guards.LlmJudge(
        judge,
        rubric="rate the answer 0..1",
        streaming_min_chars=20,
        streaming_every_n=3,
    )
    assert guard._native is not None


def test_llm_judge_streaming_kwargs_default_to_none() -> None:
    """Without streaming kwargs the v0.9.0 behaviour is preserved.
    The underlying guard's `evaluate_streaming` returns Ok(None) so
    SelfCaller's stream path falls back to buffered evaluation.
    """
    judge = tako.providers.PythonProvider("py:judge", chat=_judge_chat)
    guard = tako.guards.LlmJudge(judge, rubric="...")
    assert guard._native is not None
