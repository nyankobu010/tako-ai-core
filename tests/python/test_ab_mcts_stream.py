"""Phase 8.B — `tako.AbMcts.stream(...)` Python-facade smoke tests.

Verifies that the new async iteration surface yields the right
sequence of `OrchEvent`s through an AB-MCTS run, and that the
``verifier_score`` event variant from Phase 8.A surfaces the
expected float on the wire.
"""

from __future__ import annotations

from typing import Any

import tako


async def test_ab_mcts_stream_yields_step_text_score_then_final() -> None:
    """Single-rollout configuration: one StepStart, one AssistantText,
    one VerifierScore, one Final."""

    async def chat(_request: dict[str, Any]) -> str:
        return "this is a thirty-character response"

    provider = tako.providers.PythonProvider("py:p", chat=chat)
    # min_chars=20 ensures the rollout text passes the verifier's
    # length check, scoring 1.0 on the first iteration.
    verifier = tako.verifiers.RuleBased(min_chars=20)
    mcts = tako.AbMcts(
        provider,
        verifier,
        max_iterations=4,
        max_steps_per_rollout=1,
        min_confidence=0.95,
    )

    kinds: list[str] = []
    final_text: str | None = None
    saw_score: float | None = None
    async for ev in await mcts.stream("anything"):
        kinds.append(ev.kind)
        if ev.kind == "verifier_score":
            saw_score = ev.score
        elif ev.kind == "final":
            final_text = ev.text

    # First iteration's rollout passes min_confidence=0.95 → early-stop
    # after exactly one rollout. Expected event kinds:
    assert kinds == ["step_start", "assistant_text", "verifier_score", "final"], kinds
    assert saw_score is not None
    assert saw_score >= 0.95
    assert final_text == "this is a thirty-character response"


async def test_ab_mcts_verifier_score_event_surfaces_branch_and_score() -> None:
    """The 8.A `verifier_score` event variant must expose `branch`
    (int) and `score` (float) via the OrchEvent getters."""

    async def chat(_request: dict[str, Any]) -> str:
        return "x" * 30

    provider = tako.providers.PythonProvider("py:p", chat=chat)
    verifier = tako.verifiers.RuleBased(min_chars=20)
    mcts = tako.AbMcts(
        provider,
        verifier,
        max_iterations=2,
        max_steps_per_rollout=1,
        min_confidence=0.99,
    )

    score_events: list[tuple[int | None, float | None]] = []
    async for ev in await mcts.stream("go"):
        if ev.kind == "verifier_score":
            score_events.append((ev.branch, ev.score))

    assert len(score_events) >= 1
    branch, score = score_events[0]
    assert branch is not None
    assert score is not None
    assert 0.0 <= score <= 1.0
    # Other event variants must return None for the new getters.
    async for ev in await mcts.stream("go again"):
        if ev.kind == "step_start":
            assert ev.branch is None
            assert ev.score is None
        elif ev.kind == "final":
            assert ev.branch is None
            assert ev.score is None
            break
