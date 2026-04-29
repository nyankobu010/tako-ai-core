"""tako training harnesses.

Phase 3: Trinity router classifier training (rule-based featuriser
shared with Rust + a 2-layer MLP fitted via numpy SGD; export to ONNX
optional).

Use the CLI:

```
python -m tako.training.trinity --rollouts rollouts.jsonl --out model.onnx
```

Or the API:

```python
from tako.training.trinity import TrinityTrainer
TrinityTrainer().fit_jsonl("rollouts.jsonl").export_onnx("model.onnx")
```
"""

from __future__ import annotations

from .features import FEATURE_DIM, featurise_text
from .trinity import TrinityTrainer

__all__ = ["FEATURE_DIM", "TrinityTrainer", "featurise_text"]
