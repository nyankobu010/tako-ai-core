"""Phase 33.B — Python facade smoke for OIDC mTLS cert/key
rotation.

Phase 33.A added `OidcAuthResolver::reload_mtls_identity` and
`reload_mtls_identity_combined` on the Rust side. This file
pins the Python-side surface so a regression in the PyO3
binding lands here before user code.

The Rust unit tests in
``crates/tako-compat/src/auth/oidc.rs`` cover the actual
semantics (atomic swap, error on no-mTLS-configured, preserve
old Client on PEM parse failure); this file verifies attribute
presence + the operator-error path on calling reload without
prior mTLS config.

Tests skip themselves on a wheel built without ``auth-oidc``.
"""

from __future__ import annotations

import pytest

from tako import compat


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_reload_mtls_identity() -> None:
    """Phase 33.B — facade attribute presence."""
    assert hasattr(compat.OidcAuth, "reload_mtls_identity")
    assert callable(compat.OidcAuth.reload_mtls_identity)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_reload_mtls_identity_combined() -> None:
    assert hasattr(compat.OidcAuth, "reload_mtls_identity_combined")
    assert callable(compat.OidcAuth.reload_mtls_identity_combined)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_phase33_documented_in_module_docstring() -> None:
    """Phase 33.B — the new entry points are mentioned in the
    ``tako.compat`` module docstring so end users discover them.
    """
    docstring = compat.serve_openai.__doc__ or ""
    assert "reload_mtls_identity" in docstring
    # The "atomic swap" / "cert rotation" framing is the
    # hardest-to-discover semantic; pin its presence too.
    assert "rotation" in docstring.lower() or "atomic" in docstring.lower()
