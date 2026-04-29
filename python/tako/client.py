"""``tako.Client`` — opinionated container for the most common setup.

In Phase 1 the Client is mainly a convenience wrapper that stashes the
provider list, MCP server list, budget, and tracing config so users can
rebuild orchestrators without re-passing them. Future phases will use it
to dispatch through routers and policy engines.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from tako import mcp, tracing  # noqa: F401  -- referenced via attribute access only
    from tako.budget import Budget, InMemoryBackend, RedisBackend
    from tako.providers import _ProviderBase


class Client:
    def __init__(
        self,
        *,
        providers: list[_ProviderBase] | None = None,
        mcp_servers: list[object] | None = None,
        budget: Budget | None = None,
        budget_backend: InMemoryBackend | RedisBackend | None = None,
        tracing: object | None = None,
    ) -> None:
        self.providers = list(providers or [])
        self.mcp_servers = list(mcp_servers or [])
        self.budget = budget
        self.budget_backend = budget_backend
        self.tracing = tracing

    def __repr__(self) -> str:
        ids = [p.id for p in self.providers]
        return f"Client(providers={ids}, mcp_servers={len(self.mcp_servers)})"
