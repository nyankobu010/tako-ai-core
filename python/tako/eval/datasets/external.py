"""External dataset loaders (Phase 4).

Loads SWE-Bench-Lite (princeton-nlp/SWE-bench_Lite) and GPQA-Diamond
(Idavidrein/gpqa) from Hugging Face on-demand. Both loaders require
``pip install tako[eval]`` (the ``datasets`` and ``huggingface_hub``
packages). No data is committed to the repo.

Verification strategy is intentionally lightweight for v0.5.0:
- SWE-Bench: substring-match on filenames touched by the gold patch.
  Real SWE-Bench grading (apply patch, run repo's tests in a
  sandboxed container) is deferred to a later phase.
- GPQA: the question is asked verbatim with the four labeled choices
  appended; pass = the model's answer mentions the correct letter
  (A/B/C/D).
"""

from __future__ import annotations

import re
from typing import Any

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
) -> Dataset:
    """Load princeton-nlp/SWE-bench_Lite (300-issue benchmark)."""

    ds_pkg = _require_datasets()
    raw = ds_pkg.load_dataset("princeton-nlp/SWE-bench_Lite", split=split)
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
