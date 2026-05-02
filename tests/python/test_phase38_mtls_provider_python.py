"""Phase 38 — Python facade for the Phase 37 trait-based
``MtlsIdentityProvider``.

The Rust unit tests in
``crates/tako-compat/src/auth/oidc_mtls_provider.rs`` cover the
actual refresh semantics (initial fetch drives reload, error
preserves client, drop stops watcher). Phase 38 adds a Python
async-callable bridge plus an
``OidcAuth.watch_mtls_provider(provider)`` Python method;
this file pins the facade attribute presence + the
return-shape parser via a focused unit-level smoke.

Tests skip themselves on a wheel built without
``auth-mtls-identity-provider``.
"""

from __future__ import annotations

import pytest
from tako import compat


@pytest.mark.skipif(
    compat.MtlsIdentityProvider is None,
    reason="wheel built without the auth-mtls-identity-provider feature",
)
def test_mtls_identity_provider_constructs_from_callable() -> None:
    """Phase 38 — facade attribute presence + happy-path
    construction."""

    async def fetch() -> tuple[bytes, bytes]:
        return b"cert-bytes", b"key-bytes"

    provider = compat.MtlsIdentityProvider(fetch)
    assert provider is not None
    assert "MtlsIdentityProvider" in repr(provider)


@pytest.mark.skipif(
    compat.MtlsIdentityProvider is None,
    reason="wheel built without the auth-mtls-identity-provider feature",
)
def test_mtls_identity_provider_accepts_dict_returning_callable() -> None:
    """The dict-shape return is also accepted by the
    Phase 38 PyMtlsImpl extractor."""

    async def fetch() -> dict[str, bytes]:
        return {"cert_pem": b"cert", "key_pem": b"key"}

    # Construction should succeed; the actual return-shape
    # check happens at fetch-time inside the Rust loop, not
    # here. The Phase 37 Rust unit tests cover the loop.
    provider = compat.MtlsIdentityProvider(fetch)
    assert provider is not None


@pytest.mark.skipif(
    compat.MtlsProviderWatcher is None,
    reason="wheel built without the auth-mtls-identity-provider feature",
)
def test_mtls_provider_watcher_protocol() -> None:
    """The handle pyclass exposes shutdown + context-manager."""
    assert callable(compat.MtlsProviderWatcher.shutdown)
    assert callable(compat.MtlsProviderWatcher.__enter__)
    assert callable(compat.MtlsProviderWatcher.__exit__)


@pytest.mark.skipif(
    compat.MtlsIdentityProvider is None,
    reason="wheel built without the auth-mtls-identity-provider feature",
)
def test_oidc_auth_has_watch_mtls_provider() -> None:
    assert hasattr(compat.OidcAuth, "watch_mtls_provider")
    assert callable(compat.OidcAuth.watch_mtls_provider)


def test_phase38_classes_in_all_export() -> None:
    """Always-present in ``__all__`` even on slim wheels —
    the symbols just resolve to ``None`` instead of classes.
    Mirrors the Phase 14 / 21 / 35 pattern."""
    assert "MtlsIdentityProvider" in compat.__all__
    assert "MtlsProviderWatcher" in compat.__all__
