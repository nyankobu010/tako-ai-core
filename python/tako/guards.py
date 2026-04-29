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
    """

    def __init__(self, judge: Any, rubric: str) -> None:
        if not hasattr(judge, "_handle"):
            raise TypeError("judge must be a tako.providers.* instance")
        self._native = _native.LlmJudgeGuard(judge._handle, rubric)


__all__ = ["LlmJudge", "RuleBased"]
