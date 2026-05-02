"""Router classes for Trinity / SingleAgent (Phase 3)."""

from __future__ import annotations

from typing import Any

from tako import _native


class _RouterBase:
    """Common mixin so orchestrators can inspect ``_native``."""

    _native: Any


class RegexRouter(_RouterBase):
    """Rule-based default: maps prompt features (code/math/fallback)
    to one of the candidate providers.

    The default rules expect the candidate list to be ordered
    ``[code, math, fallback]``, but you can pass any candidate list —
    the router clamps out-of-range choices to ``len-1``.
    """

    def __init__(self) -> None:
        self._native = _native.RegexRouter()


class OnnxRouter(_RouterBase):
    """Learned router backed by an ONNX classifier.

    Available only when the wheel was built with ``--features onnx``;
    otherwise this constructor raises ``RuntimeError``. Train a model
    via :mod:`tako.training.trinity` and pass the resulting ``.onnx``
    file path here.
    """

    def __init__(self, path: str) -> None:
        cls = getattr(_native, "OnnxRouter", None)
        if cls is None:
            raise RuntimeError(
                "tako._native.OnnxRouter is unavailable; rebuild the wheel "
                "with `maturin build --features onnx` and ensure "
                "libonnxruntime is on the dynamic loader path."
            )
        self._native = cls(path)


__all__ = ["OnnxRouter", "RegexRouter"]
