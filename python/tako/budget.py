"""Budget value type."""

from __future__ import annotations

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
