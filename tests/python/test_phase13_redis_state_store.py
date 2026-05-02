"""Phase 13.A — RedisStateStore Python facade smoke tests.

Live-Redis tests are opt-in via the ``TAKO_REDIS_TESTS`` env var
(plus the wheel must be built with ``--features redis``). On a
default test run (no Redis, default wheel features) the gating
``pytest.importorskip`` and env-var skip keep this file inert.
"""

from __future__ import annotations

import os

import pytest
import tako
from tako import _native

REDIS_URL = os.environ.get("TAKO_REDIS_URL", "redis://127.0.0.1:6379")
REDIS_TESTS_ENABLED = bool(os.environ.get("TAKO_REDIS_TESTS"))

# Skip the entire module unless the wheel was built with the redis
# feature so users running the default pytest aren't forced to install
# a Redis client.
if not hasattr(_native, "RedisStateStore"):
    pytest.skip(
        "wheel built without --features redis; skipping RedisStateStore tests",
        allow_module_level=True,
    )


def test_redis_state_store_class_is_available() -> None:
    """The Python wrapper class is exported from ``tako.sigstore``
    when the wheel ships the ``redis`` feature."""
    assert hasattr(tako.sigstore, "RedisStateStore")
    assert "RedisStateStore" in tako.sigstore.__all__


@pytest.mark.skipif(
    not REDIS_TESTS_ENABLED,
    reason="set TAKO_REDIS_TESTS=1 to enable live-Redis integration tests",
)
async def test_redis_state_store_round_trip() -> None:
    key = f"tako:sigstore:test:py_round_trip:{os.getpid()}"
    store = await tako.sigstore.RedisStateStore.connect(REDIS_URL, key=key)
    await store.save(0)
    assert await store.load() == 0
    await store.save(7)
    assert await store.load() == 7


@pytest.mark.skipif(
    not REDIS_TESTS_ENABLED,
    reason="set TAKO_REDIS_TESTS=1 to enable live-Redis integration tests",
)
async def test_redis_state_store_save_is_monotonic() -> None:
    """Phase 13.A core safety property surfaced through the Python
    facade: a stale write must not clobber a higher water-mark."""
    key = f"tako:sigstore:test:py_monotonic:{os.getpid()}"
    store = await tako.sigstore.RedisStateStore.connect(REDIS_URL, key=key)
    await store.save(10)
    await store.save(5)
    assert await store.load() == 10
    await store.save(12)
    assert await store.load() == 12


@pytest.mark.skipif(
    not REDIS_TESTS_ENABLED,
    reason="set TAKO_REDIS_TESTS=1 to enable live-Redis integration tests",
)
async def test_redis_state_store_first_boot_returns_zero() -> None:
    key = f"tako:sigstore:test:py_first_boot:{os.getpid()}"
    store = await tako.sigstore.RedisStateStore.connect(REDIS_URL, key=key)
    # Without a prior save, load() returns 0 even if the key has never
    # existed — matches JsonStateStore first-boot semantics.
    assert await store.load() == 0
