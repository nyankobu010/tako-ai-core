"""Phase 45 — Python facade for the Phase 44
``OidcAuthResolver::discover_with_extra_root`` constructor.

Phase 44 (v0.45.0) shipped the Rust-side parallel constructor
that builds the resolver-wide HTTP client with an
operator-supplied PEM-encoded root CA bundle added to its
trust store. The same trust anchor covers BOTH the OIDC
discovery doc fetch AND every subsequent JWKS refresh.

The wire-level / PEM-parse / persistence semantics are
covered by the Rust unit tests in
``crates/tako-compat/src/auth/oidc.rs`` and the integration
tests in ``crates/tako-compat/tests/oidc_mtls_e2e.rs``. This
file pins the **Python-side surface** (binding name +
staticmethod nature + arg shape) so a regression in the
PyO3 wrapping lands here before user code.

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
def test_oidc_auth_has_discover_with_extra_root() -> None:
    """The Phase 44 staticmethod is wired on `OidcAuth`."""
    assert hasattr(compat.OidcAuth, "discover_with_extra_root")
    assert callable(compat.OidcAuth.discover_with_extra_root)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
async def test_discover_with_extra_root_invalid_pem_raises_value_error() -> None:
    """Awaiting the constructor with garbage CA bytes surfaces
    the Phase 44 fail-closed contract through PyO3 as a
    ``ValueError`` (mapped from ``TakoError::Invalid``).

    Proves end-to-end that:
      1. The staticmethod is actually awaitable.
      2. The ``Vec<u8>`` arg accepts ``bytes``.
      3. Errors flow through ``map_err`` correctly.

    No network call — the constructor fails synchronously
    inside the async block before any GET is attempted.
    """
    with pytest.raises(ValueError, match=r"resolver extra root CA PEM"):
        await compat.OidcAuth.discover_with_extra_root(
            "https://issuer.example",
            "test-audience",
            b"definitely not a pem certificate",
        )


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_discover_constructors_coexist() -> None:
    """Both `discover` and `discover_with_extra_root` exist
    side-by-side on the `OidcAuth` class."""
    assert hasattr(compat.OidcAuth, "discover")
    assert hasattr(compat.OidcAuth, "discover_with_extra_root")
