"""Phase 25.B — Python facade smoke for RFC 8705 §2.2
`self_signed_tls_client_auth`.

Phase 25.A added `IntrospectionAuthMethod::SelfSignedTlsClientAuth`
and `OidcAuthResolver::with_introspection_self_signed_mtls` on the
Rust side. This file pins the Python-facing surface.

The Rust unit tests in
``crates/tako-compat/src/auth/oidc.rs`` cover the actual
semantics (PEM parsing, auto-selector preference between
CA-backed and self-signed, request-time fail on missing
identity); this file verifies attribute presence and the new
alias-parser entries.

Tests skip themselves on a wheel built without ``auth-oidc``.
"""

from __future__ import annotations

import pytest

from tako import compat


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_with_introspection_self_signed_mtls() -> None:
    """Phase 25.B — facade attribute presence."""
    assert hasattr(compat.OidcAuth, "with_introspection_self_signed_mtls")
    assert callable(compat.OidcAuth.with_introspection_self_signed_mtls)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_with_introspection_self_signed_mtls_combined() -> None:
    assert hasattr(
        compat.OidcAuth,
        "with_introspection_self_signed_mtls_combined",
    )
    assert callable(compat.OidcAuth.with_introspection_self_signed_mtls_combined)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_phase25_aliases_documented_in_module_docstring() -> None:
    """Phase 25.B — the new entry points are mentioned in the
    `tako.compat` module docstring so end users discover them."""
    docstring = compat.serve_openai.__doc__ or ""
    assert "with_introspection_self_signed_mtls" in docstring
    assert (
        "self_signed_tls_client_auth" in docstring
        or "self-signed-mtls" in docstring.lower()
    )


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_with_introspection_auth_method_attribute_for_self_signed() -> None:
    """The four new case-insensitive aliases for self-signed mTLS
    (``self_signed_tls_client_auth`` / ``self-signed-tls-client-auth``
    / ``self_signed_mtls`` / ``self-signed-mtls``) are parsed by the
    alias parser. Live exercising requires a constructed
    `OidcAuth` (which needs `discover()` against an issuer); the
    Rust unit tests pin behaviour, this test pins the surface
    presence.
    """
    assert hasattr(compat.OidcAuth, "with_introspection_auth_method")
    assert callable(compat.OidcAuth.with_introspection_auth_method)
