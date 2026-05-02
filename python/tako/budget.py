"""Budget value type and backends."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from tako import _native


class Budget:
    """Per-request and per-day spend / token caps."""

    def __init__(
        self,
        *,
        max_usd_per_request: float | None = None,
        max_usd_per_day: float | None = None,
        max_tokens_per_request: int | None = None,
        max_usd_per_tenant_per_day: dict[str, float] | None = None,
    ) -> None:
        self._native: Any = _native.Budget(
            max_usd_per_request=max_usd_per_request,
            max_usd_per_day=max_usd_per_day,
            max_tokens_per_request=max_tokens_per_request,
            max_usd_per_tenant_per_day=max_usd_per_tenant_per_day,
        )

    def __repr__(self) -> str:
        return repr(self._native)


@dataclass(frozen=True)
class TenantUsage:
    """Snapshot of cumulative spend for one tenant, today (UTC)."""

    usd_today: float
    tokens_today: int


class InMemoryBackend:
    """Single-process :class:`tako_runtime::BudgetBackend`.

    Tracks per-tenant cumulative spend in memory with no day rollover —
    suitable for local dev, tests, and single-instance deployments.
    Production deployments that span processes should use
    :class:`RedisBackend` instead. ``current_usage`` and ``record`` are
    coroutines so the two backends are interchangeable at the call site.
    """

    _native: Any

    def __init__(self) -> None:
        self._native = _native.InMemoryBudgetBackend()

    async def current_usage(self, tenant_id: str) -> TenantUsage:
        """Return the cumulative usage for ``tenant_id``."""
        usd, tokens = await self._native.current_usage(tenant_id)
        return TenantUsage(usd_today=usd, tokens_today=int(tokens))

    async def record(self, tenant_id: str, usd: float, tokens: int) -> None:
        """Record ``usd`` + ``tokens`` against ``tenant_id``."""
        await self._native.record(tenant_id, usd, tokens)

    def __repr__(self) -> str:
        return repr(self._native)


class RedisBackend:
    """Redis-backed :class:`tako_runtime::BudgetBackend`.

    Stores per-tenant cumulative spend at
    ``<prefix>:{tenant_id}:{YYYY-MM-DD}`` (UTC) with auto-TTL eviction
    so day rollover is automatic. Accepts both ``redis://`` and
    ``rediss://`` URLs (the latter via rustls + webpki-roots).

    Available when the wheel was built with the ``redis`` Cargo
    feature. ``current_usage`` and ``record`` are coroutines.
    """

    _native: Any

    def __init__(
        self,
        url: str,
        *,
        key_prefix: str | None = None,
        ttl_secs: int | None = None,
    ) -> None:
        self._native = _native.RedisBudgetBackend(url, key_prefix=key_prefix, ttl_secs=ttl_secs)

    async def current_usage(self, tenant_id: str) -> TenantUsage:
        """Return the day's cumulative usage for ``tenant_id``."""
        usd, tokens = await self._native.current_usage(tenant_id)
        return TenantUsage(usd_today=usd, tokens_today=int(tokens))

    async def record(self, tenant_id: str, usd: float, tokens: int) -> None:
        """Record ``usd`` + ``tokens`` against ``tenant_id``. Atomic."""
        await self._native.record(tenant_id, usd, tokens)

    def __repr__(self) -> str:
        return repr(self._native)


__all__ = ["Budget", "InMemoryBackend", "RedisBackend", "TenantUsage"]
