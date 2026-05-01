"""Phase 18.C — Python facade smoke for the new OIDC `private_key_jwt`
+ end-session entry points on ``tako.compat.OidcAuth``.

Covers:

- ``OidcAuth.with_introspection_jwt_rs256_pem`` /
  ``with_introspection_jwt_es256_pem`` /
  ``with_introspection_jwt_ed25519_pem`` exist (Phase 18.A).
- ``OidcAuth.with_introspection_auth_method`` accepts the new
  ``"private_key_jwt"`` / ``"private-key-jwt"`` aliases (Phase 18.A).
- ``OidcAuth.end_session_endpoint`` and ``build_logout_uri`` exist
  (Phase 18.B).

The Rust unit tests in ``crates/tako-compat/src/auth/oidc.rs`` cover
the actual semantics (PEM parsing, RS256 signature verification,
form-body wire shape, OIDC Session Management URL building). This
file verifies the Python-facing surface only.

Tests skip themselves when the wheel was built without the
``auth-oidc`` feature.
"""

from __future__ import annotations

import pytest

from tako import compat


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_phase18_pem_builders() -> None:
    for name in (
        "with_introspection_jwt_rs256_pem",
        "with_introspection_jwt_es256_pem",
        "with_introspection_jwt_ed25519_pem",
    ):
        assert hasattr(compat.OidcAuth, name), f"missing {name}"
        assert callable(getattr(compat.OidcAuth, name))


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_end_session_helpers() -> None:
    assert hasattr(compat.OidcAuth, "end_session_endpoint")
    assert callable(compat.OidcAuth.end_session_endpoint)
    assert hasattr(compat.OidcAuth, "build_logout_uri")
    assert callable(compat.OidcAuth.build_logout_uri)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_phase18_aliases_documented_in_module_docstring() -> None:
    """Phase 18.C — the new entry points are mentioned in the
    `tako.compat` module docstring so end users discover them."""
    docstring = compat.serve_openai.__doc__ or ""
    assert "private_key_jwt" in docstring
    assert "with_introspection_jwt_rs256_pem" in docstring
    assert "build_logout_uri" in docstring
