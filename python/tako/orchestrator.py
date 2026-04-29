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


class Conductor:
    """Coordinator-LLM-driven multi-worker orchestrator.

    Phase 2 implementation of arXiv:2512.04388 (Sakana AI's *Conductor*).
    The coordinator emits a structured dispatch JSON at each turn; workers
    keyed by role name (e.g. ``"code"``, ``"math"``) run in parallel under
    a configurable fanout cap.
    """

    def __init__(
        self,
        coordinator: _ProviderBase,
        workers: dict[str, _ProviderBase],
        *,
        max_steps: int = 6,
        max_fanout: int = 4,
        worker_timeout_secs: int = 120,
        fail_fast: bool = False,
    ) -> None:
        if not hasattr(coordinator, "_handle"):
            raise TypeError("coordinator must be a tako.providers.* instance")
        worker_handles: dict[str, Any] = {}
        for name, w in workers.items():
            if not hasattr(w, "_handle"):
                raise TypeError(f"workers[{name!r}] must be a tako.providers.* instance")
            worker_handles[name] = w._handle
        self._inner = _native.Conductor(
            coordinator._handle,
            worker_handles,
            max_steps=max_steps,
            max_fanout=max_fanout,
            worker_timeout_secs=worker_timeout_secs,
            fail_fast=fail_fast,
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
__all__ = ["Conductor", "SingleAgent"]


def __getattr__(name: str) -> Any:
    raise AttributeError(f"tako.orchestrator has no attribute {name!r}")
