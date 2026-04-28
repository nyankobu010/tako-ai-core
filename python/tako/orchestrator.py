"""Orchestrator wrappers."""

from __future__ import annotations

from typing import Any

from tako import _native
from tako.providers import _ProviderBase


class _Result:
    """Phase-1 placeholder: orchestrators currently return the assistant
    text only. Future versions will include usage, full message, and step
    count; the field name `text` is stable."""

    __slots__ = ("text",)

    def __init__(self, text: str) -> None:
        self.text = text

    def __repr__(self) -> str:
        snippet = self.text[:60] + ("..." if len(self.text) > 60 else "")
        return f"OrchResult(text={snippet!r})"


class SingleAgent:
    """One-provider, max-step tool-call loop.

    ``mcp_servers`` accepts ``tako.mcp.Stdio`` / ``tako.mcp.Http`` instances;
    their tools are discovered via MCP's ``tools/list`` at construction time
    and merged into the orchestrator's tool registry.
    """

    def __init__(
        self,
        provider: _ProviderBase,
        *,
        max_steps: int = 8,
        mcp_servers: list[Any] | None = None,
    ) -> None:
        if not hasattr(provider, "_handle"):
            raise TypeError(
                "provider must be a tako.providers.* instance (OpenAI, Anthropic, Fake)"
            )
        native_servers: list[Any] = []
        if mcp_servers:
            for s in mcp_servers:
                if not hasattr(s, "_native"):
                    raise TypeError(
                        "mcp_servers entries must be tako.mcp.Stdio or tako.mcp.Http instances"
                    )
                native_servers.append(s._native)
        self._inner = _native.Orchestrator(
            provider._handle, max_steps, mcp_servers=native_servers or None
        )

    async def run(
        self,
        prompt: str,
        *,
        tenant_id: str | None = None,
        user_id: str | None = None,
    ) -> _Result:
        text = await self._inner.run(prompt, tenant_id=tenant_id, user_id=user_id)
        return _Result(text)

    def run_sync(
        self,
        prompt: str,
        *,
        tenant_id: str | None = None,
        user_id: str | None = None,
    ) -> _Result:
        text = self._inner.run_sync(prompt, tenant_id=tenant_id, user_id=user_id)
        return _Result(text)


# Re-export so callers can write `tako.orchestrator.SingleAgent(...)`.
__all__ = ["SingleAgent"]


def __getattr__(name: str) -> Any:
    raise AttributeError(f"tako.orchestrator has no attribute {name!r} in Phase 1")
