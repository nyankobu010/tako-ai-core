"""Confidence guards for SelfCaller (Phase 3)."""

from __future__ import annotations

from typing import Any

from tako import _native


class _GuardBase:
    """Common mixin so SelfCaller can inspect ``_native``."""

    _native: Any


class RuleBased(_GuardBase):
    """Cheap rule-based guard.

    - ``min_chars``: minimum length of the answer to count as confident.
    - ``pattern``: optional regex; if set, the answer must match.

    Returns 1.0 when both conditions hold and 0.0 otherwise.
    """

    def __init__(self, *, min_chars: int = 0, pattern: str | None = None) -> None:
        self._native = _native.RuleBasedGuard(min_chars=min_chars, pattern=pattern)


class LlmJudge(_GuardBase):
    """LLM-as-judge guard.

    Asks ``judge`` (any tako provider) to score the candidate answer
    against ``rubric`` and reply with a single decimal in ``[0, 1]``.
    Anything unparseable falls back to ``0.5``.

    Optional ``budget`` and ``budget_backend`` kwargs meter the judge's
    own provider call. This is independent of the inner orchestrator's
    budget, which covers regular execution; the judge's call goes
    out-of-band so it needs its own hook to be metered.

    Phase 9.A streaming opt-in: pass ``streaming_min_chars`` to enable
    per-N-delta judging from the streaming hook. Default is ``None``
    (streaming evaluation disabled — preserves the v0.9.0 behaviour
    where the judge runs only on buffered final text). Pair with
    ``streaming_every_n`` to throttle the judge call to every Nth
    over-threshold partial.
    """

    def __init__(
        self,
        judge: Any,
        rubric: str,
        *,
        budget: Any = None,
        budget_backend: Any = None,
        streaming_min_chars: int | None = None,
        streaming_every_n: int | None = None,
    ) -> None:
        if not hasattr(judge, "_handle"):
            raise TypeError("judge must be a tako.providers.* instance")
        budget_native = budget._native if budget is not None else None
        backend_native = budget_backend._native if budget_backend is not None else None
        self._native = _native.LlmJudgeGuard(
            judge._handle,
            rubric,
            budget=budget_native,
            budget_backend=backend_native,
            streaming_min_chars=streaming_min_chars,
            streaming_every_n=streaming_every_n,
        )


__all__ = ["LlmJudge", "RuleBased"]
