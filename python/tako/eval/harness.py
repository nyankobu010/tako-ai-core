"""Eval harness implementation.

`Dataset` and `Task` are Pydantic models. Each task carries a prompt
plus a verifier (``expected_substring`` or ``expected_regex``) and an
optional ``max_tokens`` hint. ``Eval(orch, dataset, k=...)`` runs each
task k times concurrently (bounded by ``concurrency``) and folds the
outcomes into an ``EvalReport`` (pass@k, mean USD, p50/p95 latency).

``swe_bench_lite`` and ``gpqa_diamond`` are loaded on-demand from
Hugging Face via :mod:`tako.eval.datasets.external` (requires
``pip install tako[eval]``). Verification is intentionally lightweight:
SWE-Bench uses substring-match on filenames in the gold patch; GPQA
uses an A/B/C/D positional verifier. Real SWE-Bench grading (apply
patch + run sandboxed repo tests) is deferred to a later phase.
"""

from __future__ import annotations

import argparse
import asyncio
import importlib
import re
import time
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any

from pydantic import BaseModel, ConfigDict, Field

_DATASETS_DIR = Path(__file__).parent / "datasets"


class Task(BaseModel):
    """One eval task. Either ``expected_substring`` or ``expected_regex``
    must be set; both is allowed (both must match)."""

    id: str
    prompt: str
    expected_substring: str | None = None
    expected_regex: str | None = None
    max_tokens: int | None = None

    def passes(self, output: str) -> bool:
        if self.expected_substring is not None and self.expected_substring not in output:
            return False
        if self.expected_regex is not None and not re.search(self.expected_regex, output):
            return False
        return self.expected_substring is not None or self.expected_regex is not None


class Dataset(BaseModel):
    name: str
    tasks: list[Task]


class TaskResult(BaseModel):
    task_id: str
    attempts: int
    passes: int
    elapsed_ms: list[float]
    error: str | None = None


class EvalReport(BaseModel):
    model_config = ConfigDict(populate_by_name=True)

    dataset: str
    orchestrator: str
    k: int
    tasks_run: int = Field(alias="tasks_run")
    pass_rate: float
    p50_latency_ms: float
    p95_latency_ms: float
    total_attempts: int
    task_results: list[TaskResult]


def load_synthetic() -> Dataset:
    """Load the in-tree 10-task synthetic dataset."""

    return load_jsonl(_DATASETS_DIR / "synthetic.jsonl", name="synthetic")


def load_jsonl(path: str | Path, *, name: str | None = None) -> Dataset:
    p = Path(path)
    tasks: list[Task] = []
    with p.open("r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            tasks.append(Task.model_validate_json(line))
    return Dataset(name=name or p.stem, tasks=tasks)


def load_dataset(name: str, *, limit: int | None = None) -> Dataset:
    """Load a built-in dataset by short name.

    Built-in names:
    - ``"synthetic"`` — 10-task in-tree dataset (math + factual + code).
    - ``"swe_bench_lite"`` — princeton-nlp/SWE-bench_Lite, fetched
      on-demand from Hugging Face. Requires ``pip install tako[eval]``.
    - ``"gpqa_diamond"`` — Idavidrein/gpqa (DIAMOND split). Requires
      ``pip install tako[eval]``.

    ``limit`` truncates the task list (useful for smoke runs).
    """

    if name == "synthetic":
        ds = load_synthetic()
    elif name == "swe_bench_lite":
        from .datasets.external import load_swe_bench_lite

        ds = load_swe_bench_lite(limit=limit)
    elif name == "gpqa_diamond":
        from .datasets.external import load_gpqa_diamond

        ds = load_gpqa_diamond(limit=limit)
    else:
        raise ValueError(f"unknown dataset {name!r}")

    if limit is not None and name == "synthetic":
        ds = Dataset(name=ds.name, tasks=ds.tasks[:limit])
    return ds


# An async callable that maps prompt → output text. Decouples the eval
# harness from the orchestrator class so users can plug Conductor /
# Trinity / SelfCaller / SingleAgent or any user wrapper.
RunFn = Callable[[str], Awaitable[Any]]


class Eval(BaseModel):
    """Run a dataset against an orchestrator-like callable."""

    model_config = ConfigDict(arbitrary_types_allowed=True)

    orch: Any  # tako orchestrator (must expose `.run(prompt)` -> awaitable)
    dataset: Dataset
    k: int = 1
    concurrency: int = 4
    orch_name: str = "orchestrator"

    async def run(self) -> EvalReport:
        sem = asyncio.Semaphore(self.concurrency)

        async def run_attempt(task: Task) -> tuple[Task, float, str | None, str | None]:
            async with sem:
                t0 = time.perf_counter()
                try:
                    result = await self.orch.run(task.prompt)
                except Exception as e:
                    elapsed = (time.perf_counter() - t0) * 1000.0
                    return task, elapsed, None, str(e)
                elapsed = (time.perf_counter() - t0) * 1000.0
                text = getattr(result, "text", None) or str(result)
                return task, elapsed, text, None

        coros = [run_attempt(t) for t in self.dataset.tasks for _ in range(self.k)]
        outcomes = await asyncio.gather(*coros)

        per_task: dict[str, TaskResult] = {
            t.id: TaskResult(
                task_id=t.id,
                attempts=0,
                passes=0,
                elapsed_ms=[],
                error=None,
            )
            for t in self.dataset.tasks
        }
        for task, elapsed, text, err in outcomes:
            r = per_task[task.id]
            r.attempts += 1
            r.elapsed_ms.append(elapsed)
            if err is not None:
                r.error = err
                continue
            if text is not None and task.passes(text):
                r.passes += 1

        total_attempts = sum(r.attempts for r in per_task.values())
        total_passes = sum(r.passes for r in per_task.values())
        all_latencies = [ms for r in per_task.values() for ms in r.elapsed_ms]
        p50 = _percentile(all_latencies, 50.0)
        p95 = _percentile(all_latencies, 95.0)
        return EvalReport(
            dataset=self.dataset.name,
            orchestrator=self.orch_name,
            k=self.k,
            tasks_run=len(self.dataset.tasks),
            pass_rate=(total_passes / total_attempts) if total_attempts else 0.0,
            p50_latency_ms=p50,
            p95_latency_ms=p95,
            total_attempts=total_attempts,
            task_results=list(per_task.values()),
        )


def _percentile(xs: list[float], q: float) -> float:
    if not xs:
        return 0.0
    s = sorted(xs)
    if len(s) == 1:
        return s[0]
    pos = (q / 100.0) * (len(s) - 1)
    lo = int(pos)
    hi = min(lo + 1, len(s) - 1)
    frac = pos - lo
    return s[lo] * (1 - frac) + s[hi] * frac


def _resolve_orch(spec: str) -> Any:
    """Resolve a ``module:attr`` spec to a Python object."""

    if ":" not in spec:
        raise ValueError(f"--orch must be 'module:attr', got {spec!r}")
    mod_name, attr = spec.split(":", 1)
    mod = importlib.import_module(mod_name)
    obj = getattr(mod, attr)
    return obj() if callable(obj) and not hasattr(obj, "run") else obj


def cli(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="tako.eval")
    parser.add_argument(
        "--orch",
        required=True,
        help="module:attr resolving to a tako orchestrator",
    )
    parser.add_argument("--dataset", default="synthetic", help="dataset name or .jsonl path")
    parser.add_argument("--k", type=int, default=1)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--out", default=None, help="optional JSON output path")
    args = parser.parse_args(argv)

    orch = _resolve_orch(args.orch)
    if Path(args.dataset).exists():
        dataset = load_jsonl(args.dataset)
    else:
        dataset = load_dataset(args.dataset)
    report = asyncio.run(
        Eval(
            orch=orch,
            dataset=dataset,
            k=args.k,
            concurrency=args.concurrency,
            orch_name=args.orch,
        ).run()
    )
    text = report.model_dump_json(indent=2)
    if args.out:
        Path(args.out).write_text(text, encoding="utf-8")
    else:
        print(text)
    return 0


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(cli())


__all__ = [
    "Dataset",
    "Eval",
    "EvalReport",
    "RunFn",
    "Task",
    "TaskResult",
    "cli",
    "load_dataset",
    "load_jsonl",
    "load_synthetic",
]
