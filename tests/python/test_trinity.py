"""Trinity orchestrator tests via the Python facade.

Phase 3 DoD §1: "Trinity router selects between 3 providers using a
trained ONNX model in tests" — the ONNX path requires a libonnxruntime
install; this file covers the rule-based path of the same DoD with three
distinct providers and asserts each gets called for the right prompt.
"""

from __future__ import annotations

import pytest
import tako


@pytest.fixture
def code_provider() -> tako.providers.Fake:
    return tako.providers.Fake(canned_text="<<CODE>>", id="fake:code")


@pytest.fixture
def math_provider() -> tako.providers.Fake:
    return tako.providers.Fake(canned_text="<<MATH>>", id="fake:math")


@pytest.fixture
def fallback_provider() -> tako.providers.Fake:
    return tako.providers.Fake(canned_text="<<FB>>", id="fake:fb")


def test_trinity_routes_three_providers(
    code_provider: tako.providers.Fake,
    math_provider: tako.providers.Fake,
    fallback_provider: tako.providers.Fake,
) -> None:
    """The DoD: a router picks one of three providers per prompt."""
    trinity = tako.Trinity(
        roles={
            "code": code_provider,
            "math": math_provider,
            "fallback": fallback_provider,
        },
        router=tako.routers.RegexRouter(),
        max_steps=2,
    )
    assert trinity.run_sync("Write a Rust fn to compute fib").text == "<<CODE>>"
    assert trinity.run_sync("Solve x^2 + 5 = 0").text == "<<MATH>>"
    assert trinity.run_sync("hello there").text == "<<FB>>"
    assert code_provider.call_count == 1
    assert math_provider.call_count == 1
    assert fallback_provider.call_count == 1


def test_trinity_requires_router() -> None:
    with pytest.raises(TypeError):
        tako.Trinity(
            roles={"x": tako.providers.Fake(canned_text="x")},
            router="not a router",  # type: ignore[arg-type]
        )


def test_onnx_router_unavailable_when_feature_off() -> None:
    """If the wheel was built without --features onnx, instantiating
    OnnxRouter must raise a clean RuntimeError, not a low-level error."""
    from tako import _native

    if hasattr(_native, "OnnxRouter"):
        pytest.skip("wheel built with --features onnx; skipping unavailable check")
    with pytest.raises(RuntimeError):
        tako.routers.OnnxRouter("/tmp/does-not-exist.onnx")
