"""Phase 35 — Python facade smoke for OIDC mTLS filesystem
watcher.

Phase 35 adds ``OidcAuthResolver::watch_mtls_files`` on the Rust
side plus a `MtlsFsWatcher` pyclass with context-manager
protocol. This file pins the Python-side surface so a regression
in the PyO3 binding lands here before user code.

The Rust unit tests in
``crates/tako-compat/src/auth/oidc_mtls_watcher.rs`` cover the
actual filesystem-watch semantics (cert/key change triggers
reload, parse failure preserves client, drop stops watcher);
this file just verifies attribute presence + that the wheel
exports `MtlsFsWatcher` from the matching feature gate.

Tests skip themselves on a wheel built without
``auth-mtls-fs-watch``.
"""

from __future__ import annotations

import pytest
from tako import compat


@pytest.mark.skipif(
    compat.MtlsFsWatcher is None,
    reason="wheel built without the auth-mtls-fs-watch feature",
)
def test_oidc_auth_has_watch_mtls_files() -> None:
    """Phase 35 — facade attribute presence on `OidcAuth`."""
    assert hasattr(compat.OidcAuth, "watch_mtls_files")
    assert callable(compat.OidcAuth.watch_mtls_files)


@pytest.mark.skipif(
    compat.MtlsFsWatcher is None,
    reason="wheel built without the auth-mtls-fs-watch feature",
)
def test_mtls_fs_watcher_protocol() -> None:
    """The handle pyclass exposes shutdown + context-manager."""
    assert callable(compat.MtlsFsWatcher.shutdown)
    assert callable(compat.MtlsFsWatcher.__enter__)
    assert callable(compat.MtlsFsWatcher.__exit__)


def test_mtls_fs_watcher_in_all_export() -> None:
    """`MtlsFsWatcher` is always present in `__all__` even on
    slim wheels — the symbol just resolves to `None` instead of
    a class. Mirrors the Phase 14 / 21 pattern for
    `JwtAuth` / `VaultAuth` / `ChainedAuth`.
    """
    assert "MtlsFsWatcher" in compat.__all__
