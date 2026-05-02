"""Phase 26.B — Python facade smoke for `ChainedAuth.with_short_circuit_on_transport_error`.

Phase 26.A added the fail-fast-on-transport-error opt-in flag on
the Rust side. This file pins the Python-facing surface; the
Rust unit tests in ``crates/tako-compat/src/auth/chained.rs``
cover the actual short-circuit semantics
(`short_circuit_enabled_returns_immediately_on_transport_error`,
`short_circuit_enabled_falls_through_on_invalid_error`, etc.).

`ChainedAuth` is always-on (no `auth-*` cargo feature gate), so
these tests don't need a `pytest.mark.skipif` guard.
"""

from __future__ import annotations

from tako import compat


def test_with_short_circuit_on_transport_error_returns_new_instance() -> None:
    """Immutable-builder smoke: the returned `ChainedAuth` is a
    fresh instance; the original is unchanged.
    """
    base = compat.ChainedAuth()
    flipped = base.with_short_circuit_on_transport_error()
    assert flipped is not base
    # Original is still default (no short-circuit).
    assert base.short_circuits_on_transport_error() is False
    assert flipped.short_circuits_on_transport_error() is True


def test_with_short_circuit_on_transport_error_is_idempotent() -> None:
    """Calling the builder twice doesn't break — the flag stays
    on. Mirrors the Rust-side `short_circuits_on_transport_error_accessor_reflects_state`
    test.
    """
    chain = (
        compat.ChainedAuth()
        .with_short_circuit_on_transport_error()
        .with_short_circuit_on_transport_error()
    )
    assert chain.short_circuits_on_transport_error() is True


def test_with_short_circuit_on_transport_error_preserves_children() -> None:
    """Flipping the flag doesn't drop the child list."""
    chain = (
        compat.ChainedAuth()
        .then(compat.ChainedAuth())
        .then(compat.ChainedAuth())
        .with_short_circuit_on_transport_error()
    )
    assert len(chain) == 2
    assert chain.short_circuits_on_transport_error() is True


def test_default_chain_does_not_short_circuit() -> None:
    """Phase 21 regression pin via the Python facade — default
    behaviour is fall-through-on-any-Err. The accessor reflects
    this without ever calling `with_short_circuit_on_transport_error`.
    """
    assert compat.ChainedAuth().short_circuits_on_transport_error() is False


def test_phase26_aliases_documented_in_module_docstring() -> None:
    """Phase 26.B — the new builder is mentioned in the
    `tako.compat` module docstring so end users discover it."""
    docstring = compat.serve_openai.__doc__ or ""
    assert "with_short_circuit_on_transport_error" in docstring
