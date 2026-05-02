"""External dataset loaders.

Loads SWE-Bench-Lite (princeton-nlp/SWE-bench_Lite) and GPQA-Diamond
(Idavidrein/gpqa) from Hugging Face on-demand. Both loaders require
``pip install tako[eval]`` (the ``datasets`` and ``huggingface_hub``
packages). No data is committed to the repo.

Verifier modes:

- **SWE-Bench, ``grader="filename"`` (default)** — substring-match
  on the first filename touched by the gold patch. Lightweight,
  no subprocess. Useful for smoke runs and CI.
- **SWE-Bench, ``grader="patch"`` (Phase 49)** — real grading via
  :class:`tako.eval.PatchSpec`. The model's output is treated as a
  unified diff, applied inside a fresh checkout of the SWE-Bench
  row's ``repo`` at ``base_commit``, and ``pytest`` is run on the
  ``FAIL_TO_PASS`` test ids. Requires
  ``Eval(allow_unsafe_grader=True)`` to acknowledge the
  subprocess threat model.
- **GPQA** — question + four labeled choices, A/B/C/D positional
  verifier (correct answer always presented as ``A``). No real
  grading mode; multiple-choice doesn't benefit from one.
"""

from __future__ import annotations

import json
import re
from typing import Any, Literal

from tako.eval.grader import PatchSpec
from tako.eval.harness import Dataset, Task


def _require_datasets() -> Any:
    try:
        import datasets  # type: ignore[import-not-found]
    except ImportError as e:
        raise RuntimeError(
            "Loading external datasets requires `datasets` and "
            "`huggingface_hub`. Install with `pip install tako[eval]`."
        ) from e
    return datasets


def load_swe_bench_lite(
    split: str = "test",
    *,
    limit: int | None = None,
    grader: Literal["filename", "patch"] = "filename",
) -> Dataset:
    """Load princeton-nlp/SWE-bench_Lite (300-issue benchmark).

    ``grader``:

    - ``"filename"`` (default) — substring-match on the first
      filename in the gold patch. Lightweight, no subprocess.
    - ``"patch"`` (Phase 49) — real grading. Each task carries a
      :class:`tako.eval.PatchSpec` that clones
      ``https://github.com/<row.repo>.git`` at ``row.base_commit``,
      applies the model output as a unified diff, and runs
      ``pytest`` against the row's ``FAIL_TO_PASS`` test ids.
      Use with ``Eval(allow_unsafe_grader=True)``.
    """

    ds_pkg = _require_datasets()
    raw = ds_pkg.load_dataset("princeton-nlp/SWE-bench_Lite", split=split)
    if grader == "patch":
        tasks = [_swe_bench_to_patch_task(row) for row in raw]
    else:
        tasks = [_swe_bench_to_task(row) for row in raw]
    if limit is not None:
        tasks = tasks[:limit]
    return Dataset(name="swe_bench_lite", tasks=tasks)


def load_gpqa_diamond(
    split: str = "train",
    *,
    limit: int | None = None,
) -> Dataset:
    """Load Idavidrein/gpqa (DIAMOND split, expert-curated multiple choice)."""

    ds_pkg = _require_datasets()
    raw = ds_pkg.load_dataset("Idavidrein/gpqa", "gpqa_diamond", split=split)
    tasks = [_gpqa_to_task(row) for row in raw]
    if limit is not None:
        tasks = tasks[:limit]
    return Dataset(name="gpqa_diamond", tasks=tasks)


def _swe_bench_to_task(row: dict[str, Any]) -> Task:
    """Best-effort SWE-Bench → Task. Verifier is substring-match on a
    file path the gold patch touches."""

    instance_id = str(row.get("instance_id", "<unknown>"))
    problem_statement = str(row.get("problem_statement", ""))
    repo = str(row.get("repo", ""))
    patch = str(row.get("patch", ""))

    # Heuristic: find the first `--- a/<path>` header in the patch and
    # use its filename as a substring the model is expected to mention.
    match = re.search(r"^--- a/(\S+)", patch, re.MULTILINE)
    expected = match.group(1).rsplit("/", 1)[-1] if match else None

    prompt = (
        f"Repository: {repo}\n"
        f"Issue:\n{problem_statement}\n\n"
        "Identify the file(s) most likely to need editing and explain "
        "the fix you would propose."
    )
    return Task(
        id=instance_id,
        prompt=prompt,
        expected_substring=expected,
        max_tokens=512,
    )


def _swe_bench_to_patch_task(row: dict[str, Any]) -> Task:
    """Phase 49 — real-grader SWE-Bench → Task.

    The model's output is treated as a unified diff. Grading clones
    ``https://github.com/<row.repo>.git`` at ``row.base_commit``,
    applies the diff, and runs ``pytest`` against the row's
    ``FAIL_TO_PASS`` test ids. Pass iff all those tests exit 0.

    ``FAIL_TO_PASS`` is the SWE-Bench convention for "tests the
    correct patch makes pass". For most instances it's a small
    list of pytest test ids (e.g. ``["tests/foo.py::test_bar"]``).
    The harness caps the list at 5 to bound grader runtime; any
    instance with more than 5 fail-to-pass tests gets the first
    5. Operators wanting the full list pass a custom ``test_command``
    by post-processing the loaded :class:`Dataset`.
    """

    instance_id = str(row.get("instance_id", "<unknown>"))
    problem_statement = str(row.get("problem_statement", ""))
    repo = str(row.get("repo", ""))
    base_commit = str(row.get("base_commit", ""))

    # `FAIL_TO_PASS` is JSON-encoded in the published parquet rows;
    # tolerate both string and list forms.
    fail_to_pass: Any = row.get("FAIL_TO_PASS") or "[]"
    if isinstance(fail_to_pass, str):
        try:
            fail_to_pass = json.loads(fail_to_pass)
        except (ValueError, TypeError):
            fail_to_pass = []
    if not isinstance(fail_to_pass, list):
        fail_to_pass = []
    fail_to_pass = [str(t) for t in fail_to_pass[:5]]

    spec = PatchSpec(
        repo=f"https://github.com/{repo}.git",
        base_commit=base_commit,
        # `pytest -x` stops at the first failure — useful both for
        # speed and so the log excerpt surfaces a real signal.
        test_command=["pytest", "-x", *fail_to_pass],
        apply_timeout_secs=120,
        test_timeout_secs=600,
    )

    prompt = (
        f"Repository: {repo} (commit {base_commit[:12]})\n"
        f"Issue:\n{problem_statement}\n\n"
        "Reply with a unified diff (output of `git diff`) that fixes "
        "the issue. Do not include any other prose — the response is "
        "fed directly to `git apply`."
    )
    return Task(
        id=instance_id,
        prompt=prompt,
        verify_patch=spec,
        max_tokens=4096,
    )


def _gpqa_to_task(row: dict[str, Any]) -> Task:
    """GPQA-Diamond → Task. Multiple-choice; verifier matches the
    gold letter as a standalone token in the response."""

    question = str(row.get("Question", row.get("question", "")))
    correct = str(row.get("Correct Answer", row.get("correct_answer", "")))
    incorrect_1 = str(row.get("Incorrect Answer 1", ""))
    incorrect_2 = str(row.get("Incorrect Answer 2", ""))
    incorrect_3 = str(row.get("Incorrect Answer 3", ""))
    record_id = str(row.get("Record ID", row.get("id", "<unknown>")))

    # GPQA rows don't ship with a fixed letter mapping — we present
    # answers in the order [correct, incorrect_1, ...] and tell the
    # model the correct letter is "A". The verifier then checks that
    # the response contains "A" as a standalone token.
    choices = [correct, incorrect_1, incorrect_2, incorrect_3]
    labeled = "\n".join(f"{chr(ord('A') + i)}. {c}" for i, c in enumerate(choices) if c)
    prompt = f"{question}\n\n{labeled}\n\nReply with the single letter of the correct choice."
    return Task(
        id=record_id,
        prompt=prompt,
        expected_regex=r"\bA\b",
        max_tokens=64,
    )
