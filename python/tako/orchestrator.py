"""Orchestrator wrappers."""

from __future__ import annotations

from typing import Any

from tako import _native
from tako.budget import Budget, InMemoryBackend, RedisBackend
from tako.providers import _ProviderBase

# A budget backend acceptable to ``SingleAgent``, ``Conductor``, etc.
_BudgetBackend = InMemoryBackend | RedisBackend


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

    To enable per-step provider routing, pass ``router=`` (one of the
    classes in :mod:`tako.routers`) along with ``candidates=[...]``. The
    router picks among ``[provider, *candidates]`` at each step.
    """

    def __init__(
        self,
        provider: _ProviderBase,
        *,
        max_steps: int = 8,
        mcp_servers: list[Any] | None = None,
        candidates: list[_ProviderBase] | None = None,
        router: Any | None = None,
        budget: Budget | None = None,
        budget_backend: _BudgetBackend | None = None,
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
        cand_handles: list[Any] = []
        if candidates:
            for c in candidates:
                if not hasattr(c, "_handle"):
                    raise TypeError("candidates entries must be tako.providers.* instances")
                cand_handles.append(c._handle)
        router_native = router._native if router is not None else None
        budget_native = budget._native if budget is not None else None
        backend_native = budget_backend._native if budget_backend is not None else None
        self._inner = _native.Orchestrator(
            provider._handle,
            max_steps,
            mcp_servers=native_servers or None,
            candidates=cand_handles or None,
            router=router_native,
            budget=budget_native,
            budget_backend=backend_native,
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
        budget: Any = None,
        budget_backend: Any = None,
    ) -> None:
        if not hasattr(coordinator, "_handle"):
            raise TypeError("coordinator must be a tako.providers.* instance")
        worker_handles: dict[str, Any] = {}
        for name, w in workers.items():
            if not hasattr(w, "_handle"):
                raise TypeError(f"workers[{name!r}] must be a tako.providers.* instance")
            worker_handles[name] = w._handle
        budget_native = budget._native if budget is not None else None
        backend_native = budget_backend._native if budget_backend is not None else None
        self._inner = _native.Conductor(
            coordinator._handle,
            worker_handles,
            max_steps=max_steps,
            max_fanout=max_fanout,
            worker_timeout_secs=worker_timeout_secs,
            fail_fast=fail_fast,
            budget=budget_native,
            budget_backend=backend_native,
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


class Trinity:
    """Router-driven multi-role orchestrator (Phase 3, arXiv:2512.04695).

    A :class:`Router` picks one role from a pool of
    ``role_name -> provider`` per turn. Combine with
    :class:`tako.routers.RegexRouter` for a rule-based default or
    :class:`tako.routers.OnnxRouter` for a learned classifier.
    """

    def __init__(
        self,
        roles: dict[str, _ProviderBase],
        router: Any,
        *,
        max_steps: int = 8,
        budget: Any = None,
        budget_backend: Any = None,
    ) -> None:
        if not hasattr(router, "_native"):
            raise TypeError("router must be a tako.routers.* instance")
        # Pass roles as an ordered list of (name, handle) so the Rust side
        # honours insertion order (HashMap iteration is otherwise random).
        ordered: list[tuple[str, Any]] = []
        for name, p in roles.items():
            if not hasattr(p, "_handle"):
                raise TypeError(f"roles[{name!r}] must be a tako.providers.* instance")
            ordered.append((name, p._handle))
        budget_native = budget._native if budget is not None else None
        backend_native = budget_backend._native if budget_backend is not None else None
        self._inner = _native.Trinity(
            ordered,
            router._native,
            max_steps=max_steps,
            budget=budget_native,
            budget_backend=backend_native,
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


class SelfCaller:
    """Bounded-recursion wrapper over any orchestrator (Phase 3).

    After the wrapped orchestrator emits a result, ``confidence``
    (a :class:`tako.guards.RuleBased` or :class:`tako.guards.LlmJudge`)
    scores it on ``[0, 1]``. If the score is below ``min_confidence``
    AND recursion depth is below ``max_depth``, the inner orchestrator
    is re-invoked with ``revision_prompt`` appended to the conversation.
    """

    def __init__(
        self,
        inner: SingleAgent | Conductor | Trinity,
        confidence: Any,
        *,
        max_depth: int = 3,
        min_confidence: float = 0.7,
        revision_prompt: str | None = None,
    ) -> None:
        if not hasattr(confidence, "_native"):
            raise TypeError("confidence must be a tako.guards.* instance")
        if not hasattr(inner, "_inner"):
            raise TypeError("inner must be a tako.SingleAgent / Conductor / Trinity")
        self._inner = _native.SelfCaller(
            inner._inner,
            confidence._native,
            max_depth=max_depth,
            min_confidence=min_confidence,
            revision_prompt=revision_prompt,
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

    async def stream(
        self,
        prompt: str,
        *,
        tenant_id: str | None = None,
        user_id: str | None = None,
    ) -> Any:
        """Async-iterable stream of orchestrator events (Phase 7).

        ``async for event in self_caller.stream(prompt): ...`` yields
        :class:`tako._native.OrchEvent` instances whose ``kind`` is one
        of ``"step_start" | "assistant_text" | "tool_call_start" |
        "tool_call_result" | "final"``. Inner ``final`` events from
        intermediate recursion iterations are absorbed by the
        confidence loop; the stream emits exactly one outer ``final``
        event when an iteration is accepted (or ``max_depth`` is hit).
        """
        return await self._inner.stream(prompt, tenant_id=tenant_id, user_id=user_id)


class AbMcts:
    """AB-MCTS — Adaptive Branching Monte Carlo Tree Search (Phase 4).

    Each iteration picks a branch via Thompson sampling over per-node
    Beta posteriors, rolls out one trajectory under the wrapped
    ``provider``, and updates the posteriors with a verifier-graded
    score on ``[0, 1]``. Returns the highest-scored leaf.

    ``verifier`` must be a :class:`tako.verifiers.RuleBased` (or a future
    verifier type registered with ``tako._native``).

    The Python streaming binding lands in v0.9.0: ``async for ev in
    await mcts.stream(prompt): ...`` yields per-rollout events of kind
    ``"step_start"``, ``"assistant_text"``, ``"verifier_score"``, and
    a terminal ``"final"`` carrying the best leaf's text.

    Phase 9.D (v0.10.0): pass ``candidates=[p1, p2, ...]`` and
    ``router=tako.RegexRouter()`` (or ``OnnxRouter``) to enable
    router-driven branch expansion. The router runs once per rollout
    over ``[primary, ...candidates]``; without ``router``, candidates
    are ignored and every rollout uses the primary provider.
    """

    def __init__(
        self,
        provider: _ProviderBase,
        verifier: Any,
        *,
        max_iterations: int = 16,
        branching_factor: int = 3,
        max_steps_per_rollout: int = 4,
        temperature: float = 0.7,
        min_confidence: float = 0.95,
        candidates: list[_ProviderBase] | None = None,
        router: Any = None,
    ) -> None:
        if not hasattr(provider, "_handle"):
            raise TypeError("provider must be a tako.providers.* instance")
        if not hasattr(verifier, "_native"):
            raise TypeError("verifier must be a tako.verifiers.* instance")
        cand_handles: list[Any] | None = None
        if candidates is not None:
            cand_handles = []
            for c in candidates:
                if not hasattr(c, "_handle"):
                    raise TypeError("each candidate must be a tako.providers.* instance")
                cand_handles.append(c._handle)
        router_native: Any = None
        if router is not None:
            router_native = router._native if hasattr(router, "_native") else router
        self._inner = _native.AbMcts(
            provider._handle,
            verifier._native,
            max_iterations=max_iterations,
            branching_factor=branching_factor,
            max_steps_per_rollout=max_steps_per_rollout,
            temperature=temperature,
            min_confidence=min_confidence,
            candidates=cand_handles,
            router=router_native,
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

    async def stream(
        self,
        prompt: str,
        *,
        tenant_id: str | None = None,
        user_id: str | None = None,
    ) -> Any:
        """Async-iterable stream of orchestrator events (Phase 8.B).

        Per iteration the stream yields exactly:

        - ``OrchEvent`` with ``kind="step_start"``,
        - ``OrchEvent`` with ``kind="assistant_text"`` carrying the
          rollout's full text as a single delta,
        - ``OrchEvent`` with ``kind="verifier_score"`` whose ``branch``
          and ``score`` getters expose the leaf id and verifier score
          on ``[0, 1]``.

        After all iterations (or ``min_confidence`` early-stop), one
        terminal ``OrchEvent`` with ``kind="final"`` closes the stream.
        """
        return await self._inner.stream(prompt, tenant_id=tenant_id, user_id=user_id)


# Re-export so callers can write `tako.orchestrator.SingleAgent(...)`.
__all__ = ["AbMcts", "Conductor", "SelfCaller", "SingleAgent", "Trinity"]


def __getattr__(name: str) -> Any:
    raise AttributeError(f"tako.orchestrator has no attribute {name!r}")
