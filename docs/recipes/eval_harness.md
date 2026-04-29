# Eval harness

The `tako.eval` package runs `(orchestrator, dataset)` pairs and emits
a JSON report with pass-rate, total attempts, and p50/p95 latency.

## Built-in synthetic dataset

A 10-task synthetic dataset (math + factual + code mix) ships in-tree
to satisfy Phase 3's DoD ("Eval harness runs a 10-task synthetic
benchmark and emits a JSON report"):

```python
import asyncio
import tako
from tako.eval import Eval, load_synthetic

async def main():
    fake = tako.providers.Fake(canned_text="ok hello 42 paris earth 1969 def fn")
    orch = tako.SingleAgent(provider=fake)
    report = await Eval(orch=orch, dataset=load_synthetic(), k=1).run()
    print(report.model_dump_json(indent=2))

asyncio.run(main())
```

## CLI

```bash
python -m tako.eval \
    --orch myproject.fixtures:my_orch \
    --dataset synthetic \
    --k 3 \
    --out report.json
```

`--orch` resolves a `module:attr` spec to a Python object that exposes
`.run(prompt) -> awaitable`. `--dataset` accepts `"synthetic"` or a
path to a JSONL file with `{id, prompt, expected_substring|expected_regex, max_tokens}`
rows.

## Custom datasets

```python
from tako.eval import Eval, load_jsonl

dataset = load_jsonl("path/to/eval.jsonl")
report = await Eval(orch=my_orch, dataset=dataset, k=3, concurrency=8).run()
```

`Task` requires either `expected_substring` or `expected_regex`. Both
may be set; both must match.

## External datasets

`load_dataset("swe_bench_lite")` and `load_dataset("gpqa_diamond")`
raise `NotImplementedError` — Phase 4 work. No model weights or
proprietary datasets are committed in-tree.

## Report shape

```python
class EvalReport(BaseModel):
    dataset: str
    orchestrator: str
    k: int
    tasks_run: int
    pass_rate: float
    p50_latency_ms: float
    p95_latency_ms: float
    total_attempts: int
    task_results: list[TaskResult]
```

Each `TaskResult` has `task_id`, `attempts`, `passes`, the per-attempt
latencies, and an optional `error` field if the orchestrator raised.
