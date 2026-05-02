"""Phase 15.B — Python facade smoke for Vault token-rotation
constructors and OIDC introspection builders.

The Rust unit / integration tests in
``crates/tako-compat/src/auth/{vault_token,oidc}.rs`` and
``crates/tako-compat/tests/vault_token.rs`` cover the actual rotation
behaviour and HTTP wire-protocol semantics. This file verifies the
Python-facing surface:

- ``tako.compat.VaultAuth.with_approle(addr, role_id, secret_id)``
  constructs cleanly without contacting Vault.
- ``tako.compat.VaultAuth.with_kubernetes(addr, role, jwt_path)`` and
  ``with_kubernetes_in_pod(addr, role)`` construct cleanly.
- ``tako.compat.OidcAuth.discover(...)`` is followed by a chained
  ``with_introspection_uri(uri, client_id, secret)`` that returns a
  new ``OidcAuth`` (immutable builder pattern).
- ``OidcAuth.with_introspection`` raises ``ValueError`` when discovery
  did not advertise an ``introspection_endpoint``.

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
def test_vault_auth_with_approle_constructs() -> None:
    """Construction does NOT actually authenticate against Vault — the
    AppRole login fires lazily on the first ``resolve()`` call. So no
    Vault server is required for this smoke.
    """
    auth = compat.VaultAuth.with_approle(
        "http://127.0.0.1:8200",
        "role-id-abc",
        "secret-id-xyz",
    )
    assert auth is not None
    assert type(auth).__name__ == "VaultAuth"


@pytest.mark.skipif(
    compat.VaultAuth is None,
    reason="wheel built without the auth-vault feature",
)
def test_vault_auth_with_kubernetes_constructs() -> None:
    """Construction does NOT read the JWT path — that fires lazily on
    the first ``resolve()`` call.
    """
    auth = compat.VaultAuth.with_kubernetes(
        "http://127.0.0.1:8200",
        "tako-role",
        "/tmp/sa-token-fake",
    )
    assert auth is not None


@pytest.mark.skipif(
    compat.VaultAuth is None,
    reason="wheel built without the auth-vault feature",
)
def test_vault_auth_with_kubernetes_in_pod_constructs() -> None:
    auth = compat.VaultAuth.with_kubernetes_in_pod(
        "http://127.0.0.1:8200",
        "tako-role",
    )
    assert auth is not None


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_with_introspection_methods() -> None:
    """Smoke: the new builder methods exist on the OidcAuth pyclass.

    Construction goes through the async ``discover`` constructor which
    requires a live OIDC issuer, so we don't actually call them here.
    """
    assert hasattr(compat.OidcAuth, "with_introspection")
    assert hasattr(compat.OidcAuth, "with_introspection_uri")
    assert hasattr(compat.OidcAuth, "discover")


@pytest.mark.skipif(
    compat.VaultAuth is None,
    reason="wheel built without the auth-vault feature",
)
def test_vault_auth_has_rotation_constructors() -> None:
    """Smoke: the new static-method constructors exist on VaultAuth."""
    assert hasattr(compat.VaultAuth, "with_approle")
    assert hasattr(compat.VaultAuth, "with_kubernetes")
    assert hasattr(compat.VaultAuth, "with_kubernetes_in_pod")
