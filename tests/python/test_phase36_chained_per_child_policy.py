"""Phase 36 — Python facade smoke for per-child
`ChainedAuthResolver` short-circuit policy override.

Phase 36 adds `then_with_short_circuit(child, policy)` on the
Rust `ChainedAuthResolver` and exposes it through the
`tako.compat.ChainedAuth` pyclass. This file pins the Python-side
surface so a regression in the PyO3 binding lands here before
user code.

The Rust unit tests in
``crates/tako-compat/src/auth/chained.rs`` cover the actual
short-circuit semantics (per-child override beats chain-wide,
AlwaysFallThrough overrides infra short-circuit, etc.); this file
just verifies attribute presence + the policy-string parser.
"""

from __future__ import annotations

import pytest
from tako import compat


def test_chained_auth_has_then_with_short_circuit() -> None:
    """Phase 36 — facade attribute presence."""
    assert compat.ChainedAuth is not None
    chain = compat.ChainedAuth()
    assert hasattr(chain, "then_with_short_circuit")
    assert callable(chain.then_with_short_circuit)


@pytest.mark.parametrize(
    "policy",
    [
        "inherit",
        "always_fall_through",
        "always-fall-through",
        "transport_only",
        "transport-only",
        "all_infrastructure",
        "all-infrastructure",
        "INHERIT",  # case-insensitive
        "  inherit  ",  # whitespace tolerated
    ],
)
def test_then_with_short_circuit_accepts_known_policies(policy: str) -> None:
    """Each accepted alias builds successfully against an empty
    nested ChainedAuth child (recursive composition).
    """
    chain = compat.ChainedAuth()
    nested = compat.ChainedAuth()
    result = chain.then_with_short_circuit(nested, policy)
    assert len(result) == 1


def test_then_with_short_circuit_rejects_unknown_policy() -> None:
    """A typo in the policy string raises `ValueError` listing the
    accepted aliases — not a silent fallback to a default policy.
    """
    chain = compat.ChainedAuth()
    nested = compat.ChainedAuth()
    with pytest.raises(ValueError) as exc:
        chain.then_with_short_circuit(nested, "nonexistent_policy")
    msg = str(exc.value)
    assert "nonexistent_policy" in msg
    assert "inherit" in msg
    assert "always_fall_through" in msg


def test_then_keeps_phase21_cadence() -> None:
    """Regression pin: `then(child)` still works after the Phase
    36 internal refactor that widened `Vec<Arc<dyn AuthResolver>>`
    to `Vec<ChildEntry>`.
    """
    chain = compat.ChainedAuth()
    nested = compat.ChainedAuth()
    result = chain.then(nested)
    assert len(result) == 1
