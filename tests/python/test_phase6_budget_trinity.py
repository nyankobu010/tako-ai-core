"""Phase 6.B — Trinity budget wiring (Python facade)."""

from __future__ import annotations

import asyncio
from typing import Any

import tako


async def _role_chat(_request: dict[str, Any]) -> str:
    return "role-result"


def test_trinity_accepts_budget_kwargs() -> None:
    code = tako.providers.PythonProvider("py:code", chat=_role_chat)
    fb = tako.providers.PythonProvider("py:fb", chat=_role_chat)
    backend = tako.budget.InMemoryBackend()
    trinity = tako.Trinity(
        roles={"code": code, "fallback": fb},
        router=tako.routers.RegexRouter(),
        max_steps=1,
        budget=tako.Budget(),
        budget_backend=backend,
    )

    async def _go() -> tako.budget.TenantUsage:
        await trinity.run("hi", tenant_id="tenant-trinity")
        return await backend.current_usage("tenant-trinity")

    usage = asyncio.run(_go())
    assert usage.tokens_today >= 0
