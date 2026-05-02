"""Phase 21.C — Python facade smoke for ``tako.compat.ChainedAuth``.

Composite ``AuthResolver`` that wraps N children and tries them in
append order until one returns a Principal. Phase 21.A on the Rust
side; this file covers the Python-facing surface only.

Tested:

- ``ChainedAuth`` is exposed unconditionally on
  ``tako.compat`` (no cargo feature gate).
- ``ChainedAuth()`` constructs an empty chain.
- ``then(child)`` is the immutable-builder pattern (returns a NEW
  instance) and ``len(chain)`` reflects the appended children.
  The method is named ``then(...)`` not ``with(...)`` because
  ``with`` is a Python keyword — ``chain.with(...)`` would be a
  SyntaxError.
- ``then(garbage)`` raises ``ValueError`` (the Rust side maps PyO3
  cast errors through ``extract_auth_resolver`` to ``ValueError``).
- ``ChainedAuth`` can recursively contain another ``ChainedAuth``.

The Rust unit tests in ``crates/tako-compat/src/auth/chained.rs``
remain the source of truth for behaviour (8 tests covering
short-circuit semantics, fall-through, last-error propagation,
and recursive composition).
"""

from __future__ import annotations

import pytest
from tako import compat


def test_chained_auth_attribute_exists() -> None:
    """Phase 21.B — `ChainedAuth` is always-on; every wheel exposes
    it regardless of which `auth-*` features were enabled."""
    assert compat.ChainedAuth is not None
    assert callable(compat.ChainedAuth)


def test_chained_auth_constructs_empty() -> None:
    chain = compat.ChainedAuth()
    assert chain is not None
    assert len(chain) == 0


def test_chained_auth_then_returns_new_instance() -> None:
    """Immutable-builder smoke. The Rust side's `then(child)` builds
    a fresh `ChainedAuthResolver` clone wrapped in a new pyclass, so
    the original handle is still usable.
    """
    base = compat.ChainedAuth()
    nested = base.then(compat.ChainedAuth())
    assert nested is not base
    assert len(base) == 0
    assert len(nested) == 1


def test_chained_auth_len_reflects_children_through_stacking() -> None:
    """Repeated `then` calls accumulate children. The Phase-21.A
    `chained_can_nest` test pins the recursive-composition
    behaviour on the Rust side; this verifies the Python-facing
    `__len__` hook.
    """
    chain = compat.ChainedAuth()
    for _ in range(3):
        chain = chain.then(compat.ChainedAuth())
    assert len(chain) == 3


def test_chained_auth_rejects_garbage_child() -> None:
    """`then("not a resolver")` must raise `ValueError`. The Rust
    side's `extract_auth_resolver` helper maps the cast failure
    through to a `ValueError` so the Python facade gives a clean
    error rather than a panic.
    """
    chain = compat.ChainedAuth()
    with pytest.raises(ValueError):
        chain.then("not a resolver")
    with pytest.raises(ValueError):
        chain.then(42)


def test_chained_auth_can_self_nest() -> None:
    """Recursive composition: a `ChainedAuth` whose child is itself
    a `ChainedAuth`. Useful for layered auth policies. Pinned on
    the Rust side by `chained_can_nest`.
    """
    inner = compat.ChainedAuth().then(compat.ChainedAuth())
    outer = compat.ChainedAuth().then(inner)
    assert len(outer) == 1
    assert len(inner) == 1
