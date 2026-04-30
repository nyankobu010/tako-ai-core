"""Phase 9.B — Rekor checkpoint freshness anchor (Python facade).

Each test auto-skips if the wheel was built without the ``sigstore``
feature. We assert kwarg acceptance and the high-water-mark accessor;
end-to-end Rekor checkpoint round-trips live in the Rust integration
tests in ``crates/tako-governance/tests/sigstore.rs::checkpoint_freshness``
where minting fresh keypairs at test time is straightforward.
"""

from __future__ import annotations

import pytest
from tako import _native


def _has(name: str) -> bool:
    return hasattr(_native, name)


pytestmark = pytest.mark.skipif(
    not _has("KeylessVerifier"),
    reason="wheel built without `sigstore` feature",
)


def test_keyless_verifier_accepts_rekor_min_tree_size_kwarg() -> None:
    """The new kwarg is accepted at construction; the underlying
    KeylessVerifier seeds its freshness anchor to the supplied value.
    """
    import tako.sigstore as sigstore

    v = sigstore.KeylessVerifier(
        issuer="https://accounts.example.com",
        san="https://service.example.com",
        rekor_min_tree_size=42,
    )
    assert v.rekor_max_tree_size() == 42


def test_keyless_verifier_default_freshness_zero() -> None:
    """Without a seed the high-water mark starts at 0."""
    import tako.sigstore as sigstore

    v = sigstore.KeylessVerifier(
        issuer="https://accounts.example.com",
        san="https://service.example.com",
    )
    assert v.rekor_max_tree_size() == 0
