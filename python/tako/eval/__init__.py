"""tako eval harness (Phase 3 scaffolding).

Runs ``(orchestrator, dataset)`` pairs and reports pass-rate, total
USD, and p50/p95 latency. Built-in synthetic dataset (10 tasks) ships
in-tree to satisfy Phase 3 DoD §3:

> "Eval harness runs a 10-task synthetic benchmark and emits a JSON report"

Use the CLI:

```
python -m tako.eval --orch tests.fixtures:my_orch --dataset synthetic --k 1
```

Or the API:

```python
from tako.eval import Eval, load_synthetic
report = await Eval(orch=my_orch, dataset=load_synthetic(), k=1).run()
print(report.json())
```
"""

from __future__ import annotations

from .grader import PatchSpec, grade_patch
from .harness import Dataset, Eval, EvalReport, Task, load_dataset, load_synthetic

__all__ = [
    "Dataset",
    "Eval",
    "EvalReport",
    "PatchSpec",
    "Task",
    "grade_patch",
    "load_dataset",
    "load_synthetic",
]
