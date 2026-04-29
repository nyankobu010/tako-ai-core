"""Python mirror of `tako_orchestrator::features::featurise_text`.

The Rust + Python featurisers MUST produce byte-identical f32 vectors for
the same input — the Trinity training pipeline trains in Python and
infers in Rust, so any drift would silently misroute. The unit test
`tests/python/test_features_parity.py` enforces this by feeding a fixture
corpus to both implementations and asserting equality.

Keep the rules in lockstep with `crates/tako-orchestrator/src/features.rs`.
"""

from __future__ import annotations

import math

FEATURE_DIM = 16

_CODE_KEYWORDS = (
    "fn ",
    "def ",
    "class ",
    "import ",
    "function ",
    "return ",
    "let ",
    "const ",
)

_MATH_CHARS = set("=+*/^∫∑√")


def featurise_text(text: str) -> list[float]:
    """Return a 16-dim ``list[float]`` matching the Rust featuriser."""

    f = [0.0] * FEATURE_DIM
    lower = text.lower()
    bytes_len = len(text.encode("utf-8"))
    flen = float(bytes_len)

    f[0] = math.log10(1.0 + flen)
    words = float(len(text.split()))
    f[1] = min(words / 100.0, 1.0)
    f[2] = 1.0 if "?" in text else 0.0
    f[3] = 1.0 if ("```" in text or "`" in text) else 0.0
    f[4] = 1.0 if any(kw in lower for kw in _CODE_KEYWORDS) else 0.0
    f[5] = 1.0 if any(c in _MATH_CHARS for c in text) else 0.0

    if flen > 0.0:
        digits = sum(1 for c in text.encode("utf-8") if 0x30 <= c <= 0x39)
        upper = sum(1 for c in text.encode("utf-8") if 0x41 <= c <= 0x5A)
        f[6] = digits / flen
        f[7] = upper / flen
    else:
        f[6] = 0.0
        f[7] = 0.0

    f[8] = 1.0 if "code" in lower else 0.0
    f[9] = 1.0 if ("math" in lower or "solve" in lower) else 0.0
    f[10] = 1.0 if ("explain" in lower or "describe" in lower) else 0.0
    f[11] = (
        1.0
        if ("verify" in lower or "check" in lower or "prove" in lower)
        else 0.0
    )

    newlines = float(text.count("\n"))
    f[12] = min(newlines / 20.0, 1.0)

    if flen > 0.0:
        punct = sum(
            1 for c in text.encode("utf-8") if c in (0x2E, 0x2C, 0x3B, 0x3A, 0x21)
        )
        f[13] = punct / flen
    else:
        f[13] = 0.0

    opens = text.count("(")
    closes = text.count(")")
    f[14] = 1.0 if opens > 0 and opens == closes else 0.0
    f[15] = 1.0  # bias

    return f


__all__ = ["FEATURE_DIM", "featurise_text"]
