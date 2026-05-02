"""Phase 14.A — `Verifier::evaluate_streaming` Python-side smoke for Conductor.

The Rust integration test in
``crates/tako-orchestrator/tests/conductor_streaming_verifier.rs``
covers the actual per-delta partial-event emission. This file
verifies the Python-facing surface: Conductor's existing ``verifier=``
kwarg now also surfaces streaming partial scores when the wheel-side
conductor runs in streaming mode (verified in the Rust integration
tests). ``tako.Conductor`` does not expose ``.stream()`` on the Python
facade today; like ``tako.Trinity``, streaming partials are observable
in Rust tests and (transitively) through ``tako.SelfCaller`` event
forwarding.
"""

from __future__ import annotations

from typing import Any

import tako


async def _coord_chat(_request: dict[str, Any]) -> str:
    # Halt immediately on the first turn so the Conductor.run() smoke
    # exits without needing to script worker turns.
    return '{"thought": "done", "dispatch": [], "halt": true, "final_answer": "ok"}'


async def _worker_chat(_request: dict[str, Any]) -> str:
    return "worker output"


def test_rule_based_verifier_threads_through_conductor_construct() -> None:
    """Conductor's existing ``verifier=`` kwarg now also surfaces
    streaming partial scores when the wheel-side conductor dispatches
    streaming-capable workers (verified in the Rust integration tests).
    Python-facing API is additive — no public surface change.
    """
    coordinator = tako.providers.PythonProvider("py:coord", chat=_coord_chat)
    worker = tako.providers.PythonProvider("py:worker", chat=_worker_chat)
    verifier = tako.verifiers.RuleBased(min_chars=5)

    cond = tako.Conductor(
        coordinator=coordinator,
        workers={"code": worker},
        max_steps=2,
        verifier=verifier,
    )
    assert cond is not None


async def test_rule_based_verifier_run_path_unchanged_on_conductor() -> None:
    """The non-streaming ``run`` path on Conductor still returns a
    final result text; the Phase 14.A streaming-verifier wiring lives
    on ``stream()`` only, so the synchronous coordinator-halt flow is
    unchanged.
    """
    coordinator = tako.providers.PythonProvider("py:coord", chat=_coord_chat)
    worker = tako.providers.PythonProvider("py:worker", chat=_worker_chat)
    cond = tako.Conductor(
        coordinator=coordinator,
        workers={"code": worker},
        max_steps=1,
        verifier=tako.verifiers.RuleBased(min_chars=5),
    )
    result = await cond.run("anything")
    assert result.text == "ok"
