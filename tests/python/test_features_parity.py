"""Featuriser parity: Rust featurise == Python featurise.

The training harness fits a model in Python; the OnnxRouter runs
inference in Rust. The two featurisers MUST match byte-for-byte for
trained models to score the same prompts the same way at inference.
"""

from __future__ import annotations

import math

import pytest
from tako import _native
from tako.training.features import FEATURE_DIM, featurise_text

CORPUS = [
    "",
    "hi",
    "Solve x^2 + 5x + 6 = 0",
    "Please show me code in Rust.\n```rust\nfn x() {}\n```",
    "Explain CRDTs to me.",
    "VERIFY THIS PROOF: 2+2=4.",
    "Multi-line\nprompt\nwith\nnewlines.",
    "Question? Yes!",
    "(parens balanced) and unbalanced (",
    "long " * 200,
]


@pytest.mark.parametrize("text", CORPUS)
def test_featurise_python_matches_rust(text: str) -> None:
    py = featurise_text(text)
    rs = _native.featurise_text(text)
    assert len(py) == FEATURE_DIM
    assert len(rs) == FEATURE_DIM
    for i, (a, b) in enumerate(zip(py, rs, strict=True)):
        # Rust returns f32; allow 1e-6 tolerance for floating-point ops
        # like log10. Equality of "no diff at all" is the goal.
        assert math.isclose(a, b, abs_tol=1e-6), (
            f"feature[{i}] mismatch on text={text!r}: py={a} rs={b}"
        )
