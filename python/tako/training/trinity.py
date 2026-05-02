"""Trinity router training harness (Phase 3).

Generates rollouts (one prompt x multiple candidate providers), scores
them with a verifier, fits a tiny 2-layer MLP via numpy SGD, and
optionally exports to ONNX.

Wheel-slim: `numpy` is required only when this module is actually used
(import-time guarded). `onnx` (the python package) is required only for
ONNX export — install with `pip install tako[training]`.
"""

from __future__ import annotations

import argparse
import json
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .features import FEATURE_DIM, featurise_text


@dataclass
class Rollout:
    """One labelled training example.

    `features` is precomputed (call `featurise_text(prompt)`) so the
    training pipeline never accidentally drifts from the Rust featuriser.
    """

    prompt: str
    label: int  # candidate index in `[0, n_classes)`
    features: list[float] = field(default_factory=list)

    @classmethod
    def from_prompt(cls, prompt: str, label: int) -> Rollout:
        return cls(prompt=prompt, label=label, features=featurise_text(prompt))


def label_from_scores(scores: list[float]) -> int:
    """argmax over per-candidate scores (helper for verifier loops)."""

    return max(range(len(scores)), key=lambda i: scores[i])


@dataclass
class TrinityTrainer:
    """Tiny 2-layer MLP classifier over the shared featuriser.

    Architecture: ``input(FEATURE_DIM) → hidden(H, ReLU) → logits(K)``.
    Trained with vanilla mini-batch SGD on cross-entropy. K is inferred
    from rollouts at fit time.
    """

    hidden: int = 32
    epochs: int = 200
    lr: float = 0.05
    batch_size: int = 32
    seed: int = 42
    weights: dict[str, Any] = field(default_factory=dict)
    n_classes: int = 0

    def fit(self, rollouts: list[Rollout]) -> TrinityTrainer:
        try:
            import numpy as np  # type: ignore[import-not-found]
        except ImportError as e:
            raise RuntimeError(
                "TrinityTrainer.fit requires numpy. Install with `pip install tako[training]`."
            ) from e

        if not rollouts:
            raise ValueError("rollouts is empty")
        n_classes = max(r.label for r in rollouts) + 1
        self.n_classes = n_classes
        rng = np.random.default_rng(self.seed)

        X = np.asarray([r.features for r in rollouts], dtype=np.float32)
        y = np.asarray([r.label for r in rollouts], dtype=np.int64)
        n, d = X.shape
        assert d == FEATURE_DIM, f"expected {FEATURE_DIM} features, got {d}"

        # Xavier init for the two layers.
        W1 = rng.standard_normal((d, self.hidden)).astype(np.float32) * (
            (2.0 / (d + self.hidden)) ** 0.5
        )
        b1 = np.zeros((self.hidden,), dtype=np.float32)
        W2 = rng.standard_normal((self.hidden, n_classes)).astype(np.float32) * (
            (2.0 / (self.hidden + n_classes)) ** 0.5
        )
        b2 = np.zeros((n_classes,), dtype=np.float32)

        for _ in range(self.epochs):
            order = rng.permutation(n)
            for start in range(0, n, self.batch_size):
                idx = order[start : start + self.batch_size]
                xb = X[idx]
                yb = y[idx]
                # Forward
                h_pre = xb @ W1 + b1
                h = np.maximum(h_pre, 0.0)
                logits = h @ W2 + b2
                # Softmax + cross-entropy gradient
                logits -= logits.max(axis=1, keepdims=True)
                exp = np.exp(logits)
                probs = exp / exp.sum(axis=1, keepdims=True)
                grad_logits = probs.copy()
                grad_logits[np.arange(len(yb)), yb] -= 1.0
                grad_logits /= len(yb)
                # Backprop
                grad_W2 = h.T @ grad_logits
                grad_b2 = grad_logits.sum(axis=0)
                grad_h = grad_logits @ W2.T
                grad_h[h_pre <= 0.0] = 0.0
                grad_W1 = xb.T @ grad_h
                grad_b1 = grad_h.sum(axis=0)
                # SGD step
                W2 -= self.lr * grad_W2
                b2 -= self.lr * grad_b2
                W1 -= self.lr * grad_W1
                b1 -= self.lr * grad_b1

        self.weights = {
            "W1": W1.tolist(),
            "b1": b1.tolist(),
            "W2": W2.tolist(),
            "b2": b2.tolist(),
        }
        return self

    def fit_jsonl(self, path: str | Path) -> TrinityTrainer:
        """Read a JSONL file of `{prompt: str, label: int}` lines."""

        rolls: list[Rollout] = []
        with Path(path).open("r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                obj = json.loads(line)
                rolls.append(Rollout.from_prompt(obj["prompt"], int(obj["label"])))
        return self.fit(rolls)

    def predict(self, prompt: str) -> int:
        try:
            import numpy as np  # type: ignore[import-not-found]
        except ImportError as e:
            raise RuntimeError("TrinityTrainer.predict requires numpy") from e
        if not self.weights:
            raise RuntimeError("call fit() before predict()")
        x = np.asarray(featurise_text(prompt), dtype=np.float32).reshape(1, -1)
        W1 = np.asarray(self.weights["W1"], dtype=np.float32)
        b1 = np.asarray(self.weights["b1"], dtype=np.float32)
        W2 = np.asarray(self.weights["W2"], dtype=np.float32)
        b2 = np.asarray(self.weights["b2"], dtype=np.float32)
        h = np.maximum(x @ W1 + b1, 0.0)
        logits = h @ W2 + b2
        return int(logits.argmax(axis=1)[0])

    def export_onnx(self, path: str | Path) -> None:
        """Save the trained weights as an ONNX classifier consumable by
        :class:`tako.routers.OnnxRouter`.

        The exported graph takes a `float32[1, FEATURE_DIM]` input named
        ``features`` and emits a `float32[1, n_classes]` output named
        ``logits``. Architecture matches the trainer: MatMul → Add → Relu
        → MatMul → Add.
        """

        if not self.weights:
            raise RuntimeError("call fit() before export_onnx()")
        try:
            import numpy as np  # type: ignore[import-not-found]
            import onnx  # type: ignore[import-not-found]
            from onnx import TensorProto, helper, numpy_helper
        except ImportError as e:
            raise RuntimeError(
                "export_onnx requires `onnx` and `numpy`. "
                "Install with `pip install tako[training]`."
            ) from e

        W1 = np.asarray(self.weights["W1"], dtype=np.float32)
        b1 = np.asarray(self.weights["b1"], dtype=np.float32)
        W2 = np.asarray(self.weights["W2"], dtype=np.float32)
        b2 = np.asarray(self.weights["b2"], dtype=np.float32)

        inp = helper.make_tensor_value_info("features", TensorProto.FLOAT, [1, FEATURE_DIM])
        out = helper.make_tensor_value_info("logits", TensorProto.FLOAT, [1, self.n_classes])

        init_W1 = numpy_helper.from_array(W1, name="W1")
        init_b1 = numpy_helper.from_array(b1, name="b1")
        init_W2 = numpy_helper.from_array(W2, name="W2")
        init_b2 = numpy_helper.from_array(b2, name="b2")

        nodes = [
            helper.make_node("MatMul", ["features", "W1"], ["h_pre_mat"]),
            helper.make_node("Add", ["h_pre_mat", "b1"], ["h_pre"]),
            helper.make_node("Relu", ["h_pre"], ["h"]),
            helper.make_node("MatMul", ["h", "W2"], ["logits_mat"]),
            helper.make_node("Add", ["logits_mat", "b2"], ["logits"]),
        ]
        graph = helper.make_graph(
            nodes,
            "tako-trinity",
            [inp],
            [out],
            initializer=[init_W1, init_b1, init_W2, init_b2],
        )
        model = helper.make_model(
            graph,
            producer_name="tako-training",
            opset_imports=[helper.make_opsetid("", 17)],
        )
        onnx.save(model, str(path))


def cli(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="tako.training.trinity")
    parser.add_argument("--rollouts", required=True, help="path to a JSONL training set")
    parser.add_argument("--out", required=True, help="path to the .onnx output")
    parser.add_argument("--hidden", type=int, default=32)
    parser.add_argument("--epochs", type=int, default=200)
    parser.add_argument("--lr", type=float, default=0.05)
    parser.add_argument("--seed", type=int, default=42)
    args = parser.parse_args(argv)

    trainer = TrinityTrainer(
        hidden=args.hidden,
        epochs=args.epochs,
        lr=args.lr,
        seed=args.seed,
    )
    trainer.fit_jsonl(args.rollouts)
    trainer.export_onnx(args.out)
    print(f"trained ({trainer.n_classes} classes) → {args.out}")
    return 0


if __name__ == "__main__":  # pragma: no cover — CLI entrypoint
    raise SystemExit(cli())


__all__: list[str] = ["Rollout", "TrinityTrainer", "cli", "label_from_scores"]


# Helper: a minimal verifier-style scoring callable for tests.
ScoreFn = Callable[[str, list[str]], list[float]]
