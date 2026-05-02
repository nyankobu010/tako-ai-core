"""Eval harness tests.

Phase 3 DoD §3: "Eval harness runs a 10-task synthetic benchmark and
emits a JSON report" — covered by ``test_synthetic_runs_10_tasks_and_emits_report``.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
import tako
from tako.eval import Eval, EvalReport, Task, load_synthetic


def test_synthetic_dataset_has_10_tasks() -> None:
    ds = load_synthetic()
    assert ds.name == "synthetic"
    assert len(ds.tasks) == 10
    # Every task has a verifier set.
    for t in ds.tasks:
        assert t.expected_substring or t.expected_regex


def test_task_passes_substring_and_regex() -> None:
    t = Task(id="x", prompt="?", expected_substring="42")
    assert t.passes("the answer is 42")
    assert not t.passes("forty-two")
    t2 = Task(id="y", prompt="?", expected_regex=r"(?i)\bok\b")
    assert t2.passes("OK")
    assert t2.passes("ok then")
    assert not t2.passes("nope")
    # No verifier ⇒ never passes.
    t3 = Task(id="z", prompt="?")
    assert not t3.passes("anything")


@pytest.mark.asyncio
async def test_synthetic_runs_10_tasks_and_emits_report() -> None:
    """Use a Fake provider that ALWAYS replies with text containing all
    the synthetic dataset's expected tokens. This is enough to drive
    the harness through k=1 over 10 tasks and produce a report."""

    canned = "4 42 25 paris earth 1969 def fn ok hello"
    fake = tako.providers.Fake(canned_text=canned, id="fake:eval")
    orch = tako.SingleAgent(provider=fake, max_steps=1)
    ds = load_synthetic()
    report = await Eval(orch=orch, dataset=ds, k=1, concurrency=4).run()

    assert isinstance(report, EvalReport)
    assert report.tasks_run == 10
    assert report.total_attempts == 10
    assert report.pass_rate == 1.0
    assert report.p50_latency_ms >= 0
    # Each task result has exactly k attempts.
    for r in report.task_results:
        assert r.attempts == 1


@pytest.mark.asyncio
async def test_report_serialises_to_json(tmp_path: Path) -> None:
    fake = tako.providers.Fake(canned_text="ok hello", id="fake:e")
    orch = tako.SingleAgent(provider=fake, max_steps=1)
    ds = load_synthetic()
    report = await Eval(orch=orch, dataset=ds, k=1).run()
    out = tmp_path / "report.json"
    out.write_text(report.model_dump_json(indent=2), encoding="utf-8")
    parsed = json.loads(out.read_text())
    assert parsed["tasks_run"] == 10
    assert "pass_rate" in parsed
    assert "task_results" in parsed


def test_unknown_dataset_raises() -> None:
    from tako.eval import load_dataset

    with pytest.raises(ValueError):
        load_dataset("does_not_exist")


def test_external_loaders_require_datasets_extra(monkeypatch: pytest.MonkeyPatch) -> None:
    """When ``datasets`` isn't installed, the external loaders raise a
    helpful RuntimeError pointing at ``pip install tako[eval]``."""

    import importlib
    import sys

    from tako.eval.datasets import external as ext_mod

    # Hide the `datasets` package even if it happens to be installed.
    monkeypatch.setitem(sys.modules, "datasets", None)
    importlib.reload(ext_mod)
    with pytest.raises(RuntimeError, match=r"tako\[eval\]"):
        ext_mod.load_swe_bench_lite()
    with pytest.raises(RuntimeError, match=r"tako\[eval\]"):
        ext_mod.load_gpqa_diamond()


def test_swe_bench_to_task_extracts_filename() -> None:
    from tako.eval.datasets.external import _swe_bench_to_task

    row = {
        "instance_id": "django__django-12345",
        "repo": "django/django",
        "problem_statement": "Bug in queryset filtering",
        "patch": (
            "diff --git a/django/db/models/query.py b/django/db/models/query.py\n"
            "--- a/django/db/models/query.py\n"
            "+++ b/django/db/models/query.py\n"
            "@@ -1 +1 @@\n"
            "-old\n+new\n"
        ),
    }
    task = _swe_bench_to_task(row)
    assert task.id == "django__django-12345"
    assert task.expected_substring == "query.py"
    assert "django/django" in task.prompt
    assert "Bug in queryset filtering" in task.prompt


def test_gpqa_to_task_uses_letter_a_for_correct() -> None:
    from tako.eval.datasets.external import _gpqa_to_task

    row = {
        "Record ID": "gpqa-001",
        "Question": "What is 2+2?",
        "Correct Answer": "4",
        "Incorrect Answer 1": "3",
        "Incorrect Answer 2": "5",
        "Incorrect Answer 3": "22",
    }
    task = _gpqa_to_task(row)
    assert task.id == "gpqa-001"
    assert task.expected_regex == r"\bA\b"
    # Correct answer is presented as choice A
    assert "A. 4" in task.prompt
    assert task.passes("The answer is A.")
    assert not task.passes("The answer is B.")
