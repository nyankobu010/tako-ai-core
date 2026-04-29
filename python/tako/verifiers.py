"""Verifiers for AB-MCTS (Phase 8)."""

from __future__ import annotations

from typing import Any

from tako import _native


class _VerifierBase:
    """Common mixin so :class:`tako.AbMcts` can inspect ``_native``."""

    _native: Any


class RuleBased(_VerifierBase):
    """Cheap rule-based verifier.

    - ``min_chars``: minimum length the rollout text must reach to score
      as fully passing (1.0). Below this length, the verifier returns a
      partial score proportional to ``len(text) / min_chars``.
    - ``pattern``: optional regex; if set and matched, score is 1.0; if
      set and unmatched, score is 0.0 regardless of length.
    """

    def __init__(self, *, min_chars: int = 0, pattern: str | None = None) -> None:
        self._native = _native.RuleBasedVerifier(min_chars=min_chars, pattern=pattern)


__all__ = ["RuleBased"]
