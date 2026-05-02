"""Phase 40 — Python facade for the Phase 39 ``MtlsRefreshHook``.

The Rust unit + integration tests in
``crates/tako-compat/src/auth/oidc.rs`` /
``oidc_mtls_hook.rs`` /
``oidc_mtls_watcher.rs`` /
``oidc_mtls_provider.rs`` cover the actual retry semantics
(predicate, retry fires when conditions met, retry skipped
without hook / for non-mTLS methods, hook protocol). This
file pins the Python-side surface so a regression in the PyO3
binding lands here before user code.

Tests skip themselves on a wheel built without the matching
``auth-*`` features.
"""

from __future__ import annotations

import pytest
from tako import compat


@pytest.mark.skipif(
    compat.MtlsRefreshHook is None,
    reason="wheel built without the auth-oidc feature",
)
def test_mtls_refresh_hook_class_exposed() -> None:
    """Phase 40 — facade attribute presence + repr."""
    assert compat.MtlsRefreshHook is not None
    assert "MtlsRefreshHook" in repr(compat.MtlsRefreshHook)


@pytest.mark.skipif(
    compat.OidcAuth is None,
    reason="wheel built without the auth-oidc feature",
)
def test_oidc_auth_has_with_mtls_refresh_hook() -> None:
    """The Phase 40 builder is wired on `OidcAuth`."""
    assert hasattr(compat.OidcAuth, "with_mtls_refresh_hook")
    assert callable(compat.OidcAuth.with_mtls_refresh_hook)


@pytest.mark.skipif(
    compat.MtlsFsWatcher is None,
    reason="wheel built without the auth-mtls-fs-watch feature",
)
def test_mtls_fs_watcher_has_refresh_hook() -> None:
    """`MtlsFsWatcher.refresh_hook()` is the Phase 35 source."""
    assert hasattr(compat.MtlsFsWatcher, "refresh_hook")
    assert callable(compat.MtlsFsWatcher.refresh_hook)


@pytest.mark.skipif(
    compat.MtlsProviderWatcher is None,
    reason="wheel built without the auth-mtls-identity-provider feature",
)
def test_mtls_provider_watcher_has_refresh_hook() -> None:
    """`MtlsProviderWatcher.refresh_hook()` is the Phase 37/38 source."""
    assert hasattr(compat.MtlsProviderWatcher, "refresh_hook")
    assert callable(compat.MtlsProviderWatcher.refresh_hook)


def test_mtls_refresh_hook_in_all_export() -> None:
    """Always-present in ``__all__`` even on slim wheels —
    the symbol just resolves to ``None`` instead of a class.
    Mirrors the Phase 14 / 21 / 35 / 38 pattern."""
    assert "MtlsRefreshHook" in compat.__all__
