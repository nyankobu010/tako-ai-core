"""SelfCaller orchestrator tests via the Python facade.

Phase 3 DoD §2: "SelfCaller terminates within `max_depth` on adversarial
inputs" — covered by ``test_terminates_within_max_depth_on_adversarial``.
"""

from __future__ import annotations

import pytest
import tako


def test_self_caller_passes_through_when_confidence_meets_threshold() -> None:
    fake = tako.providers.Fake(canned_text="long enough answer for the rule", id="fake:p")
    inner = tako.SingleAgent(provider=fake, max_steps=1)
    sc = tako.SelfCaller(
        inner=inner,
        confidence=tako.guards.RuleBased(min_chars=5),
        max_depth=3,
        min_confidence=0.5,
    )
    result = sc.run_sync("hello")
    assert "long enough" in result.text
    # Only one provider call; no recursion.
    assert fake.call_count == 1


def test_terminates_within_max_depth_on_adversarial() -> None:
    """Adversarial guard: always returns 0.0 (output never confident).
    SelfCaller must stop after exactly ``max_depth + 1`` inner runs and
    return the last output, not loop forever."""
    fake = tako.providers.Fake(canned_text="too short", id="fake:p")
    inner = tako.SingleAgent(provider=fake, max_steps=1)

    # min_chars=999 means the rule-based guard returns 0.0 for any
    # canned_text the Fake provider emits — adversarial.
    sc = tako.SelfCaller(
        inner=inner,
        confidence=tako.guards.RuleBased(min_chars=999),
        max_depth=2,
        min_confidence=0.5,
    )
    result = sc.run_sync("anything")
    assert result.text  # not empty — it returns the final low-confidence output
    # max_depth=2 means inner is called at offsets 0, 1, 2 → 3 times.
    assert fake.call_count == 3


async def test_self_caller_recurses_until_threshold() -> None:
    """The third response is long enough; first two are not. Guard's
    ``min_chars=20`` accepts only the third. Uses PythonProvider for
    multi-turn cycling, which requires an asyncio event loop (run_sync
    is incompatible with Python provider dispatch)."""

    state = {"idx": 0}
    responses = [
        "x",
        "still short",
        "this answer is twenty-plus characters long for sure",
    ]

    async def chat(_request: dict) -> str:
        text = responses[min(state["idx"], len(responses) - 1)]
        state["idx"] += 1
        return text

    provider = tako.providers.PythonProvider(id="py:scripted", chat=chat)
    inner = tako.SingleAgent(provider=provider, max_steps=1)
    sc = tako.SelfCaller(
        inner=inner,
        confidence=tako.guards.RuleBased(min_chars=20),
        max_depth=5,
        min_confidence=0.5,
    )
    result = await sc.run("explain CRDTs")
    assert "twenty-plus" in result.text
    assert state["idx"] == 3


def test_self_caller_rejects_non_orchestrator() -> None:
    with pytest.raises(TypeError):
        tako.SelfCaller(
            inner="not orchestrator",  # type: ignore[arg-type]
            confidence=tako.guards.RuleBased(min_chars=1),
        )


def test_self_caller_rejects_non_guard() -> None:
    fake = tako.providers.Fake(canned_text="x")
    inner = tako.SingleAgent(provider=fake, max_steps=1)
    with pytest.raises(TypeError):
        tako.SelfCaller(inner=inner, confidence="not a guard")  # type: ignore[arg-type]
