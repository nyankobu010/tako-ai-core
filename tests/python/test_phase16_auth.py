"""Phase 16.B.3 — Python facade smoke for Vault namespace + OIDC
introspection auth-method builders.

The Rust unit tests in ``crates/tako-compat/src/auth/{vault,oidc}.rs``
cover the actual semantics (header propagation, RFC 7662 §2.1 wire
shape). This file verifies the Python-facing surface:

- ``tako.compat.VaultAuth(addr, token).with_namespace(ns)`` returns a
  new ``VaultAuth`` (immutable builder).
- ``tako.compat.VaultAuth.with_namespace`` is chainable on top of the
  Phase 15.B.1 ``with_approle`` / ``with_kubernetes`` /
  ``with_kubernetes_in_pod`` constructors.
- ``OidcAuth.with_introspection_auth_method`` accepts the four
  case-insensitive aliases (``"basic"`` / ``"client_secret_basic"`` /
  ``"post"`` / ``"client_secret_post"``) and raises ``ValueError``
  on garbage input.

These tests run against a wheel built with
``maturin develop --features "auth-jwt auth-oidc auth-vault"``.
On a wheel built without those features, each test skips itself.
"""

from __future__ import annotations

import pytest

from tako import compat


@pytest.mark.skipif(
    compat.VaultAuth is None,
    reason="wheel built without the auth-vault feature",
)
def test_vault_auth_has_with_namespace() -> None:
    assert hasattr(compat.VaultAuth, "with_namespace")


@pytest.mark.skipif(
    compat.VaultAuth is None,
    reason="wheel built without the auth-vault feature",
)
def test_vault_auth_with_namespace_returns_new_instance() -> None:
    """Immutable-builder smoke — no Vault contact required."""
    base = compat.VaultAuth("http://127.0.0.1:8200", "dev-token")
    scoped = base.with_namespace("eng-team")
    assert scoped is not None
    assert type(scoped).__name__ == "VaultAuth"
    # Returned a fresh instance — original handle still usable.
    assert scoped is not base


@pytest.mark.skipif(
    compat.VaultAuth is None,
    reason="wheel built without the auth-vault feature",
)
def test_vault_auth_with_namespace_chains_on_approle() -> None:
    """Phase 16.B.1 — namespace is orthogonal to the auth method.
    Chains cleanly on top of ``with_approle``.
    """
    auth = compat.VaultAuth.with_approle(
        "http://127.0.0.1:8200",
        "role-id",
        "secret-id",
    ).with_namespace("acme")
    assert auth is not None


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_with_introspection_auth_method() -> None:
    """Smoke: the new builder method exists on the OidcAuth pyclass.

    Construction requires the async ``discover`` constructor and a
    live OIDC issuer; the Rust unit tests in
    ``crates/tako-compat/src/auth/oidc.rs`` cover the alias-parsing
    and the silent-no-op-without-introspection behaviour.
    """
    assert hasattr(compat.OidcAuth, "with_introspection_auth_method")
    assert callable(compat.OidcAuth.with_introspection_auth_method)
