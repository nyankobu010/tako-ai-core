"""Phase 27.B — Python facade smoke for `ChainedAuth.with_short_circuit_on_infrastructure_errors`.

Phase 27.A added the broader infrastructure-error short-circuit
policy on the Rust side, refactoring the Phase-26 boolean to a
`ShortCircuitPolicy` enum with three states: `None` /
`TransportOnly` / `AllInfrastructure`. This file pins the
Python-facing surface; the Rust unit tests in
``crates/tako-compat/src/auth/chained.rs`` cover the actual
short-circuit semantics across the four infrastructure variants
(`Transport` / `RateLimited` / `CircuitOpen` /
`BudgetExhausted`).

`ChainedAuth` is always-on (no `auth-*` cargo feature gate), so
these tests don't need a `pytest.mark.skipif` guard.
"""

from __future__ import annotations

from tako import compat


def test_with_short_circuit_on_infrastructure_errors_returns_new_instance() -> None:
    """Immutable-builder smoke: the returned `ChainedAuth` is a
    fresh instance; the original is unchanged."""
    base = compat.ChainedAuth()
    flipped = base.with_short_circuit_on_infrastructure_errors()
    assert flipped is not base
    # Original is still default (no short-circuit on either).
    assert base.short_circuits_on_transport_error() is False
    assert base.short_circuits_on_infrastructure_errors() is False
    # Flipped chain has both accessors true (broader policy
    # supersets narrower).
    assert flipped.short_circuits_on_transport_error() is True
    assert flipped.short_circuits_on_infrastructure_errors() is True


def test_short_circuits_on_infrastructure_errors_accessor_default() -> None:
    """Default chain has neither short-circuit policy."""
    chain = compat.ChainedAuth()
    assert chain.short_circuits_on_infrastructure_errors() is False


def test_short_circuit_policy_is_last_write_wins() -> None:
    """Last-write-wins between the Phase-26 narrower and Phase-27
    broader builders. Calling broader after narrower upgrades;
    calling narrower after broader downgrades. Mirrors the
    Rust-side `short_circuit_policy_is_last_write_wins` test.
    """
    # narrower → broader: upgrades
    chain = (
        compat.ChainedAuth()
        .with_short_circuit_on_transport_error()
        .with_short_circuit_on_infrastructure_errors()
    )
    assert chain.short_circuits_on_infrastructure_errors() is True

    # broader → narrower: downgrades
    chain = (
        compat.ChainedAuth()
        .with_short_circuit_on_infrastructure_errors()
        .with_short_circuit_on_transport_error()
    )
    assert chain.short_circuits_on_infrastructure_errors() is False
    # But transport short-circuit is still active.
    assert chain.short_circuits_on_transport_error() is True


def test_phase26_narrower_does_not_set_infrastructure_accessor() -> None:
    """Regression pin: the Phase-26 narrower flag does NOT flip
    the `short_circuits_on_infrastructure_errors` accessor to
    True, even though both flags share the same internal
    `ShortCircuitPolicy` enum after the Phase-27 refactor.
    """
    chain = compat.ChainedAuth().with_short_circuit_on_transport_error()
    assert chain.short_circuits_on_transport_error() is True
    assert chain.short_circuits_on_infrastructure_errors() is False


def test_with_short_circuit_on_infrastructure_errors_is_idempotent() -> None:
    chain = (
        compat.ChainedAuth()
        .with_short_circuit_on_infrastructure_errors()
        .with_short_circuit_on_infrastructure_errors()
    )
    assert chain.short_circuits_on_infrastructure_errors() is True


def test_phase27_aliases_documented_in_module_docstring() -> None:
    """Phase 27.B — the new builder is mentioned in the
    `tako.compat` module docstring so end users discover it."""
    docstring = compat.serve_openai.__doc__ or ""
    assert "with_short_circuit_on_infrastructure_errors" in docstring
