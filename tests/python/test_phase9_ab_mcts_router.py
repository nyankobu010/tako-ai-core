"""Phase 9.D — AbMcts router-driven branch expansion (Python facade).

End-to-end Rust integration tests in
``crates/tako-orchestrator/tests/ab_mcts.rs::branch_routing`` cover
branch dispatch and the no-router regression. This smoke confirms
the kwargs round-trip through `tako.AbMcts` and that runs continue
to work without a router (Phase 9.0 backwards-compat).
"""

from __future__ import annotations

from typing import Any

import pytest
import tako
from tako import _native


async def _fake_chat(_request: dict[str, Any]) -> str:
    return "rollout text long enough"


def test_ab_mcts_accepts_candidates_and_router_kwargs() -> None:
    """Constructing AbMcts with candidates + router round-trips
    through to the Rust builder. RegexRouter is always available
    (no Cargo feature gate)."""
    primary = tako.providers.PythonProvider("py:p0", chat=_fake_chat)
    candidate = tako.providers.PythonProvider("py:p1", chat=_fake_chat)
    verifier = tako.verifiers.RuleBased(min_chars=5)
    router = _native.RegexRouter()

    mcts = tako.AbMcts(
        primary,
        verifier,
        max_iterations=2,
        max_steps_per_rollout=1,
        candidates=[candidate],
        router=router,
    )
    assert mcts is not None


def test_ab_mcts_rejects_non_provider_candidate() -> None:
    primary = tako.providers.PythonProvider("py:p0", chat=_fake_chat)
    verifier = tako.verifiers.RuleBased(min_chars=5)
    with pytest.raises(TypeError):
        tako.AbMcts(primary, verifier, candidates=["not a provider"])


@pytest.mark.asyncio
async def test_ab_mcts_runs_without_router_unchanged_from_v090() -> None:
    """No-router build is identical to the v0.9.0 path: the candidate
    is registered but never invoked; the primary handles every rollout.
    """
    primary = tako.providers.PythonProvider("py:p0", chat=_fake_chat)
    candidate = tako.providers.PythonProvider("py:p1", chat=_fake_chat)
    verifier = tako.verifiers.RuleBased(min_chars=5)

    mcts = tako.AbMcts(
        primary,
        verifier,
        max_iterations=2,
        max_steps_per_rollout=1,
        min_confidence=0.99,
        candidates=[candidate],
    )
    result = await mcts.run("anything")
    assert result.text == "rollout text long enough"
