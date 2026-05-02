"""Phase 13.B — Verifier::evaluate_streaming Python-side smoke.

The Rust integration test in
``crates/tako-orchestrator/tests/trinity.rs::streaming_verifier_emits``
covers the actual per-delta partial-event emission. This file
verifies the Python-facing surface: the shipped
``tako.verifiers.RuleBased`` (whose Rust ``RuleBasedVerifier`` now
overrides ``evaluate_streaming``) constructs cleanly and threads
through ``tako.Trinity(verifier=...)`` without breaking the existing
non-streaming ``run()`` path. ``tako.Trinity`` does not expose
``.stream()`` on the Python facade today; the streaming partials are
observable in Rust integration tests and (transitively) through
``tako.SelfCaller(inner=trinity).stream(...)`` which forwards inner
events.
"""

from __future__ import annotations

from typing import Any

import tako


async def _fake_chat(_request: dict[str, Any]) -> str:
    return "trinity assistant text"


def test_rule_based_verifier_constructs_with_streaming_override() -> None:
    """The Rust ``RuleBasedVerifier`` shipped in Phase 8 now overrides
    ``Verifier::evaluate_streaming`` (Phase 13.B); construction from
    the Python facade is unchanged.
    """
    v = tako.verifiers.RuleBased(min_chars=10)
    assert v._native is not None


def test_rule_based_verifier_threads_through_trinity_construct() -> None:
    """Trinity's existing ``verifier=`` kwarg now also surfaces
    streaming partial scores when the wheel-side trinity runs in
    streaming mode (verified in the Rust integration tests). This
    smoke just confirms construction stays additive — no Python
    surface change.
    """
    primary = tako.providers.PythonProvider("py:p0", chat=_fake_chat)
    fallback = tako.providers.PythonProvider("py:p1", chat=_fake_chat)
    router = tako.routers.RegexRouter()
    verifier = tako.verifiers.RuleBased(min_chars=5)

    trinity = tako.Trinity(
        roles={"primary": primary, "fallback": fallback},
        router=router,
        max_steps=2,
        verifier=verifier,
    )
    assert trinity is not None


async def test_rule_based_verifier_run_path_unchanged() -> None:
    """The non-streaming ``run`` path on Trinity still returns a
    final result text; verifier streaming does not perturb it."""
    primary = tako.providers.PythonProvider("py:primary", chat=_fake_chat)
    router = tako.routers.RegexRouter()
    trinity = tako.Trinity(
        roles={"primary": primary},
        router=router,
        max_steps=1,
        verifier=tako.verifiers.RuleBased(min_chars=5),
    )
    result = await trinity.run("anything")
    assert result.text == "trinity assistant text"
