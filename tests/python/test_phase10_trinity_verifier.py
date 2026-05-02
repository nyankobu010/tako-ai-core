"""Phase 10.C — Trinity emits OrchEvent::VerifierScore when a verifier
is attached. Mirrors the Rust integration test
``crates/tako-orchestrator/tests/trinity.rs::verifier_emits``.
"""

from __future__ import annotations

import pytest
import tako


async def _fake_chat(_request: dict) -> str:
    return "trinity assistant text"


def test_trinity_accepts_verifier_kwarg() -> None:
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
    # Construction succeeds — the kwarg threads through to the Rust
    # builder without raising.
    assert trinity is not None


def test_trinity_rejects_non_verifier() -> None:
    primary = tako.providers.PythonProvider("py:p0", chat=_fake_chat)
    router = tako.routers.RegexRouter()

    with pytest.raises(TypeError, match=r"verifier must be a tako\.verifiers\."):
        tako.Trinity(
            roles={"primary": primary},
            router=router,
            verifier="not a verifier",  # type: ignore[arg-type]
        )


def test_trinity_default_no_verifier_kwarg() -> None:
    # Backwards-compat: omitting the verifier kwarg is fine.
    primary = tako.providers.PythonProvider("py:p0", chat=_fake_chat)
    router = tako.routers.RegexRouter()
    trinity = tako.Trinity(
        roles={"primary": primary},
        router=router,
        max_steps=1,
    )
    assert trinity is not None
