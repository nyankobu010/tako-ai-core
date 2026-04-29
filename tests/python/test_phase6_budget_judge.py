"""Phase 6.C — LlmJudgeGuard budget wiring (Python facade)."""

from __future__ import annotations

from typing import Any

import tako


async def _judge_chat(_request: dict[str, Any]) -> str:
    return "0.85"


def test_llm_judge_accepts_budget_kwargs() -> None:
    judge = tako.providers.PythonProvider("py:judge", chat=_judge_chat)
    backend = tako.budget.InMemoryBackend()

    # Build the guard via the public facade. Just constructing it is
    # enough to verify the kwargs are accepted; the guard's own budget
    # only fires when it's evaluated, which happens inside SelfCaller.
    guard = tako.guards.LlmJudge(
        judge,
        rubric="rate confidence on a scale of 0-1",
        budget=tako.Budget(),
        budget_backend=backend,
    )
    assert guard._native is not None
