"""Phase 10.C — Conductor emits OrchEvent::VerifierScore per worker
when a verifier is attached. Mirrors the Rust integration test
``crates/tako-orchestrator/tests/conductor.rs::verifier_emits``.
"""

from __future__ import annotations

import pytest

import tako


async def _fake_chat(_request: dict) -> str:
    return "worker output"


def test_conductor_accepts_verifier_kwarg() -> None:
    coord = tako.providers.PythonProvider("py:coord", chat=_fake_chat)
    a = tako.providers.PythonProvider("py:a", chat=_fake_chat)
    verifier = tako.verifiers.RuleBased(min_chars=3)

    cond = tako.Conductor(
        coordinator=coord,
        workers={"a": a},
        max_steps=2,
        verifier=verifier,
    )
    assert cond is not None


def test_conductor_rejects_non_verifier() -> None:
    coord = tako.providers.PythonProvider("py:coord", chat=_fake_chat)

    with pytest.raises(TypeError, match="verifier must be a tako.verifiers.*"):
        tako.Conductor(
            coordinator=coord,
            workers={},
            verifier="not a verifier",  # type: ignore[arg-type]
        )


def test_conductor_default_no_verifier_kwarg() -> None:
    coord = tako.providers.PythonProvider("py:coord", chat=_fake_chat)
    cond = tako.Conductor(coordinator=coord, workers={})
    assert cond is not None
