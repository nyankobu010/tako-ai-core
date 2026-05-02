"""Training harness smoke tests.

These tests skip cleanly when `numpy` (and `onnx` for export) are not
installed — the wheel ships without them and they live behind the
`tako[training]` extra.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from tako.training.features import FEATURE_DIM, featurise_text
from tako.training.trinity import Rollout, TrinityTrainer

pytestmark = pytest.mark.filterwarnings("ignore::DeprecationWarning")


def test_rollout_from_prompt() -> None:
    r = Rollout.from_prompt("Solve 2+2", label=1)
    assert len(r.features) == FEATURE_DIM
    assert r.features == featurise_text("Solve 2+2")


def test_trainer_fits_and_predicts() -> None:
    pytest.importorskip("numpy")
    rolls = [
        Rollout.from_prompt("Write a fn in Rust", label=0),
        Rollout.from_prompt("Code me a function", label=0),
        Rollout.from_prompt("def hello():\n    return 1", label=0),
        Rollout.from_prompt("Solve x + 1 = 2", label=1),
        Rollout.from_prompt("Compute 12 / 4", label=1),
        Rollout.from_prompt("What is 2 + 2", label=1),
        Rollout.from_prompt("hello there friend", label=2),
        Rollout.from_prompt("how are you doing today", label=2),
        Rollout.from_prompt("nice to meet you", label=2),
    ]
    trainer = TrinityTrainer(epochs=300, lr=0.1, seed=0).fit(rolls)
    # Trainer should classify a clearly-code prompt as 0, math as 1, chat as 2.
    assert trainer.predict("Write a fn that returns 42") == 0
    assert trainer.predict("Solve 5 + 7") == 1


def test_trainer_export_onnx(tmp_path: Path) -> None:
    pytest.importorskip("numpy")
    pytest.importorskip("onnx")
    rolls = [
        Rollout.from_prompt("Code", label=0),
        Rollout.from_prompt("Solve", label=1),
        Rollout.from_prompt("Hi", label=2),
    ]
    trainer = TrinityTrainer(epochs=10, seed=0).fit(rolls)
    out = tmp_path / "model.onnx"
    trainer.export_onnx(out)
    assert out.exists() and out.stat().st_size > 100


def test_fit_jsonl(tmp_path: Path) -> None:
    pytest.importorskip("numpy")
    p = tmp_path / "rollouts.jsonl"
    p.write_text(
        "\n".join(
            json.dumps({"prompt": pr, "label": lbl})
            for pr, lbl in [
                ("write a fn", 0),
                ("solve 2+2", 1),
                ("hello", 2),
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    trainer = TrinityTrainer(epochs=5).fit_jsonl(p)
    assert trainer.n_classes == 3
    assert "W1" in trainer.weights
