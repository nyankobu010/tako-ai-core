"""Phase 43 — Python facade for the Phase 42 ``_extra_root`` mTLS
introspection builders.

Phase 42 (v0.43.0) shipped two new Rust builders on
``OidcAuthResolver`` that load a client cert + private key AND
add an operator-supplied PEM-encoded root CA bundle to the
underlying HTTP client's trust store:

- ``with_introspection_mtls_extra_root(cert, key, extra_ca)``
  (RFC 8705 ``tls_client_auth``).
- ``with_introspection_self_signed_mtls_extra_root(cert, key, extra_ca)``
  (RFC 8705 §2.2 ``self_signed_tls_client_auth``).

Plus the ``IntrospectionConfig::extra_root_ca_pem`` field that
persists the bundle so the rotation surfaces (Phase 33 / 35 /
37 / 39) re-apply the same trust anchors after a cert/key swap.

The wire-level / PEM-parse / persistence semantics are covered
by the Rust unit tests in ``crates/tako-compat/src/auth/oidc.rs``
and the integration tests in
``crates/tako-compat/tests/oidc_mtls_e2e.rs``. This file pins
the **Python-side surface** (binding name + arg shape +
callability) so a regression in the PyO3 wrapping lands here
before user code.

Tests skip themselves on a wheel built without the
``auth-oidc`` feature.
"""

from __future__ import annotations

import pytest
from tako import compat


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_with_introspection_mtls_extra_root() -> None:
    """The Phase 42 ``tls_client_auth`` builder is wired on `OidcAuth`."""
    assert hasattr(compat.OidcAuth, "with_introspection_mtls_extra_root")
    assert callable(compat.OidcAuth.with_introspection_mtls_extra_root)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_with_introspection_self_signed_mtls_extra_root() -> None:
    """The Phase 42 ``self_signed_tls_client_auth`` builder is wired."""
    assert hasattr(compat.OidcAuth, "with_introspection_self_signed_mtls_extra_root")
    assert callable(compat.OidcAuth.with_introspection_self_signed_mtls_extra_root)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_extra_root_builders_present_alongside_phase_24_25_siblings() -> None:
    """All four mTLS builders coexist on the `OidcAuth` class."""
    for name in (
        "with_introspection_mtls",
        "with_introspection_mtls_combined",
        "with_introspection_self_signed_mtls",
        "with_introspection_self_signed_mtls_combined",
        "with_introspection_mtls_extra_root",
        "with_introspection_self_signed_mtls_extra_root",
    ):
        assert hasattr(compat.OidcAuth, name), f"OidcAuth is missing the {name!r} builder method"
