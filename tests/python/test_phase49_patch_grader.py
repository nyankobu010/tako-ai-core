"""Phase 49 — real eval harness patch grader.

Closes the last open backlog item in ``PLAN.md``: until now,
SWE-Bench-style "fix this issue" tasks were graded by
substring-matching a filename in the model's prose output.
Phase 49 adds a real grader that:

1. Clones a target repo at a specific SHA into a temp dir.
2. Applies the model's output as a unified diff (``git apply``).
3. Runs an operator-supplied test command (``pytest`` for
   SWE-Bench).
4. Passes iff the test exits 0.

These tests use a tiny self-contained ``git`` repo built in
``tmp_path`` rather than reaching out to GitHub. ``git`` and
``pytest`` are required on PATH (CI already has them).
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

import pytest
import tako
from tako.eval import Eval, PatchSpec, Task, grade_patch
from tako.eval.harness import Dataset

# All tests need git to drive the fixture.
pytestmark = pytest.mark.skipif(
    shutil.which("git") is None,
    reason="phase 49 grader tests require git on PATH",
)


# ---------------------------------------------------------------------------
# Fixture: a self-contained git repo with a buggy module + a pytest test
# that fails until a known patch is applied.
# ---------------------------------------------------------------------------


_BUGGY_MODULE = '''\
"""Phase 49 grader fixture — buggy module."""

def add(a: int, b: int) -> int:
    # Deliberately wrong: returns a-b instead of a+b. The fix
    # patch in the test below replaces this with the correct
    # expression.
    return a - b
'''


_PYTEST_FILE = '''\
"""Phase 49 grader fixture — failing test until the fix lands."""

from module import add


def test_add_two_plus_three_equals_five() -> None:
    assert add(2, 3) == 5
'''


def _git(args: list[str], *, cwd: Path) -> None:
    subprocess.run(
        ["git", *args],
        cwd=str(cwd),
        check=True,
        capture_output=True,
    )


def _git_out(args: list[str], *, cwd: Path) -> str:
    # `encoding="utf-8"` is critical on Windows: without it
    # `text=True` decodes git's stdout using the platform default
    # (cp1252 on GHA Windows runners), which mojibakes any
    # non-ASCII bytes — e.g. the em dash (`—`, UTF-8
    # `\xe2\x80\x94`) in `_BUGGY_MODULE`'s docstring becomes `â€"`.
    # The corrupted patch's context lines then don't match the
    # (correctly UTF-8) working tree and `git apply --check`
    # reports `patch does not apply` at the line containing the
    # em dash.
    return subprocess.run(
        ["git", *args],
        cwd=str(cwd),
        capture_output=True,
        check=True,
        text=True,
        encoding="utf-8",
    ).stdout


@pytest.fixture
def buggy_repo(tmp_path: Path) -> tuple[Path, str, str]:
    """Build a fresh git repo with a failing test, then generate
    a real ``git diff`` that fixes it. Returns
    ``(repo_path, base_commit_sha, fix_patch)``.

    The diff is generated rather than hand-crafted because
    ``git apply --check`` is strict about hunk-header line counts
    and trailing whitespace; hand-crafting is brittle.
    """

    repo = tmp_path / "buggy"
    repo.mkdir()
    _git(["init", "--quiet", "--initial-branch=main"], cwd=repo)
    _git(["config", "user.email", "phase49@example.com"], cwd=repo)
    _git(["config", "user.name", "Phase49"], cwd=repo)
    (repo / "module.py").write_text(_BUGGY_MODULE, encoding="utf-8")
    (repo / "test_module.py").write_text(_PYTEST_FILE, encoding="utf-8")
    _git(["add", "."], cwd=repo)
    _git(["commit", "--quiet", "-m", "initial buggy state"], cwd=repo)
    sha = _git_out(["rev-parse", "HEAD"], cwd=repo).strip()

    # Apply the fix in-place, capture `git diff`, then revert so
    # the working tree matches the committed (buggy) state. The
    # captured diff is what we feed to the grader.
    module_path = repo / "module.py"
    buggy = module_path.read_text(encoding="utf-8")
    module_path.write_text(buggy.replace("a - b", "a + b"), encoding="utf-8")
    fix_patch = _git_out(["diff", "module.py"], cwd=repo)
    module_path.write_text(buggy, encoding="utf-8")
    return repo, sha, fix_patch


# ---------------------------------------------------------------------------
# PatchSpec model.
# ---------------------------------------------------------------------------


def test_patch_spec_validates_required_fields() -> None:
    spec = PatchSpec(
        repo="file:///tmp/x",
        base_commit="abc123",
        test_command=["pytest", "-x"],
    )
    assert spec.repo == "file:///tmp/x"
    assert spec.apply_timeout_secs == 60  # default
    assert spec.test_timeout_secs == 300  # default


def test_patch_spec_rejects_extra_invalid_types() -> None:
    from pydantic import ValidationError

    with pytest.raises(ValidationError):
        PatchSpec(repo=42, base_commit="x", test_command=[])  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
# grade_patch — happy path / edge cases.
# ---------------------------------------------------------------------------


@pytest.mark.skipif(
    shutil.which("pytest") is None,
    reason="grader integration test requires pytest on PATH",
)
async def test_grade_patch_passes_when_fix_makes_test_green(
    buggy_repo: tuple[Path, str, str],
) -> None:
    repo_path, sha, fix_patch = buggy_repo
    spec = PatchSpec(
        repo=f"file://{repo_path}",
        base_commit=sha,
        test_command=[sys.executable, "-m", "pytest", "-x", "test_module.py"],
        apply_timeout_secs=30,
        test_timeout_secs=60,
    )
    ok, log = await grade_patch(spec, fix_patch)
    assert ok, f"expected the fix patch to pass; log: {log}"


async def test_grade_patch_rejects_non_applying_patch(
    buggy_repo: tuple[Path, str, str],
) -> None:
    """A patch targeting a file that doesn't exist must fail at
    ``git apply --check`` time, before any test runs."""
    repo_path, sha, _fix = buggy_repo
    spec = PatchSpec(
        repo=f"file://{repo_path}",
        base_commit=sha,
        # Using `python -c "pass"` as a no-op test — if the apply
        # check ever passes, this would make the grader return
        # `True`. We assert the opposite: the apply step fails
        # first, so the test never runs.
        test_command=[sys.executable, "-c", "pass"],
        apply_timeout_secs=30,
        test_timeout_secs=30,
    )
    bogus = (
        "diff --git a/no_such_file.py b/no_such_file.py\n"
        "--- a/no_such_file.py\n"
        "+++ b/no_such_file.py\n"
        "@@ -1 +1 @@\n"
        "-one\n+two\n"
    )
    ok, log = await grade_patch(spec, bogus)
    assert not ok
    assert "git apply" in log.lower()


@pytest.mark.skipif(
    shutil.which("pytest") is None,
    reason="grader integration test requires pytest on PATH",
)
async def test_grade_patch_fails_when_test_still_fails(
    buggy_repo: tuple[Path, str, str],
) -> None:
    """Patch applies cleanly but the test still fails — grader
    must report False with the test log."""
    repo_path, sha, _fix = buggy_repo
    spec = PatchSpec(
        repo=f"file://{repo_path}",
        base_commit=sha,
        test_command=[sys.executable, "-m", "pytest", "-x", "test_module.py"],
        apply_timeout_secs=30,
        test_timeout_secs=60,
    )
    # Generate a "doesn't fix the bug" patch dynamically: tweak
    # only the docstring, leave `a - b` intact. Generating via
    # `git diff` keeps the hunk header well-formed.
    module_path = repo_path / "module.py"
    original = module_path.read_text(encoding="utf-8")
    module_path.write_text(
        original.replace(
            'buggy module."""',
            'buggy module (annotated)."""',
        ),
        encoding="utf-8",
    )
    no_op_patch = _git_out(["diff", "module.py"], cwd=repo_path)
    module_path.write_text(original, encoding="utf-8")

    ok, log = await grade_patch(spec, no_op_patch)
    assert not ok
    # pytest exit codes: 1 = test failures.
    assert "test exit=" in log


async def test_grade_patch_apply_timeout_returns_false(
    buggy_repo: tuple[Path, str, str],
) -> None:
    """``apply_timeout_secs=0`` is too aggressive for any real
    work; the clone or apply step times out and the grader
    fails fast."""
    repo_path, sha, fix_patch = buggy_repo
    spec = PatchSpec(
        repo=f"file://{repo_path}",
        base_commit=sha,
        test_command=[sys.executable, "-c", "pass"],
        apply_timeout_secs=0,
        test_timeout_secs=30,
    )
    ok, _log = await grade_patch(spec, fix_patch)
    assert not ok


async def test_grade_patch_empty_test_command_fails() -> None:
    spec = PatchSpec(
        repo="file:///tmp/x",
        base_commit="abc",
        test_command=[],
    )
    ok, log = await grade_patch(spec, "irrelevant")
    assert not ok
    assert "test_command is empty" in log


# ---------------------------------------------------------------------------
# Task.verify_async dispatch.
# ---------------------------------------------------------------------------


async def test_task_verify_async_substring_path() -> None:
    t = Task(id="x", prompt="?", expected_substring="42")
    assert await t.verify_async("the answer is 42")
    assert not await t.verify_async("forty-two")


async def test_task_verify_async_patch_path_dispatches_to_grader(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """``verify_async`` must route to ``grade_patch`` when the
    task carries a ``verify_patch`` spec — no need to actually
    run git for this dispatch test."""

    captured: dict[str, object] = {}

    async def fake_grade(spec: PatchSpec, output: str) -> tuple[bool, str]:
        captured["spec"] = spec
        captured["output"] = output
        return True, "fake-pass"

    # The harness imports `grade_patch` at module import time, so
    # patch it on the harness module too.
    monkeypatch.setattr("tako.eval.grader.grade_patch", fake_grade)
    monkeypatch.setattr("tako.eval.harness.grade_patch", fake_grade)

    spec = PatchSpec(
        repo="file:///tmp/x",
        base_commit="abc",
        test_command=["pytest"],
    )
    t = Task(id="x", prompt="?", verify_patch=spec)
    assert await t.verify_async("a unified diff")
    assert captured["output"] == "a unified diff"
    assert captured["spec"] is spec


def test_task_passes_returns_false_for_patch_task() -> None:
    """Sync ``passes()`` is used by callers that haven't migrated
    to ``verify_async``; for a patch task it must fail closed
    rather than silently returning True with no verification."""
    spec = PatchSpec(repo="file:///tmp/x", base_commit="abc", test_command=["pytest"])
    t = Task(id="x", prompt="?", verify_patch=spec)
    assert not t.passes("any output")


# ---------------------------------------------------------------------------
# Eval security gate.
# ---------------------------------------------------------------------------


async def test_eval_rejects_patch_dataset_without_unsafe_flag() -> None:
    spec = PatchSpec(repo="file:///tmp/x", base_commit="abc", test_command=["pytest"])
    ds = Dataset(name="d", tasks=[Task(id="t", prompt="?", verify_patch=spec)])
    fake = tako.providers.Fake(canned_text="diff", id="fake:e")
    orch = tako.SingleAgent(provider=fake, max_steps=1)
    with pytest.raises(ValueError, match=r"allow_unsafe_grader"):
        await Eval(orch=orch, dataset=ds, k=1).run()


async def test_eval_runs_with_unsafe_flag_set(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """With the gate flipped, the eval runs; verifier dispatches
    to a stubbed grader so this test stays self-contained."""

    async def fake_grade(spec: PatchSpec, output: str) -> tuple[bool, str]:
        return True, "stub"

    monkeypatch.setattr("tako.eval.grader.grade_patch", fake_grade)
    monkeypatch.setattr("tako.eval.harness.grade_patch", fake_grade)

    spec = PatchSpec(repo="file:///tmp/x", base_commit="abc", test_command=["pytest"])
    ds = Dataset(name="d", tasks=[Task(id="t", prompt="?", verify_patch=spec)])
    fake = tako.providers.Fake(canned_text="diff", id="fake:e")
    orch = tako.SingleAgent(provider=fake, max_steps=1)
    report = await Eval(orch=orch, dataset=ds, k=1, allow_unsafe_grader=True).run()
    assert report.tasks_run == 1
    assert report.pass_rate == 1.0


# ---------------------------------------------------------------------------
# SWE-Bench grader="patch" adapter.
# ---------------------------------------------------------------------------


def test_swe_bench_to_patch_task_builds_correct_spec() -> None:
    from tako.eval.datasets.external import _swe_bench_to_patch_task

    row = {
        "instance_id": "django__django-12345",
        "repo": "django/django",
        "base_commit": "abcdef0123456789",
        "problem_statement": "Bug in queryset filtering",
        "FAIL_TO_PASS": '["tests/db/test_query.py::test_filter"]',
    }
    task = _swe_bench_to_patch_task(row)
    assert task.id == "django__django-12345"
    assert task.verify_patch is not None
    spec = task.verify_patch
    assert spec.repo == "https://github.com/django/django.git"
    assert spec.base_commit == "abcdef0123456789"
    assert spec.test_command[0] == "pytest"
    assert spec.test_command[1] == "-x"
    assert "tests/db/test_query.py::test_filter" in spec.test_command
    assert "git diff" in task.prompt or "unified diff" in task.prompt


def test_swe_bench_to_patch_task_caps_fail_to_pass_at_five() -> None:
    from tako.eval.datasets.external import _swe_bench_to_patch_task

    fail_to_pass = [f"tests/test_{i}.py::test_x" for i in range(20)]
    import json as _json

    row = {
        "instance_id": "x",
        "repo": "owner/repo",
        "base_commit": "deadbeef",
        "problem_statement": "...",
        "FAIL_TO_PASS": _json.dumps(fail_to_pass),
    }
    task = _swe_bench_to_patch_task(row)
    spec = task.verify_patch
    assert spec is not None
    # `pytest -x <up to 5 ids>` = 7 args max.
    assert len(spec.test_command) <= 7


def test_swe_bench_to_patch_task_tolerates_missing_fail_to_pass() -> None:
    from tako.eval.datasets.external import _swe_bench_to_patch_task

    row = {
        "instance_id": "x",
        "repo": "owner/repo",
        "base_commit": "deadbeef",
        "problem_statement": "...",
    }
    task = _swe_bench_to_patch_task(row)
    spec = task.verify_patch
    assert spec is not None
    # No FAIL_TO_PASS → just `pytest -x`.
    assert spec.test_command == ["pytest", "-x"]


def test_load_swe_bench_lite_grader_param_routes_to_correct_adapter(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """``load_swe_bench_lite(grader="patch")`` produces tasks
    with ``verify_patch`` set; the default produces tasks with
    ``expected_substring`` set."""

    from tako.eval.datasets import external as ext_mod

    fake_rows = [
        {
            "instance_id": "x",
            "repo": "owner/repo",
            "base_commit": "abc",
            "problem_statement": "...",
            "patch": (
                "diff --git a/file.py b/file.py\n"
                "--- a/file.py\n"
                "+++ b/file.py\n"
                "@@ -1 +1 @@\n-a\n+b\n"
            ),
            "FAIL_TO_PASS": '["tests/test.py::t"]',
        }
    ]

    class _FakeDatasets:
        @staticmethod
        def load_dataset(*_args: object, **_kwargs: object) -> list[dict]:
            return fake_rows

    monkeypatch.setattr(ext_mod, "_require_datasets", lambda: _FakeDatasets)

    ds_filename = ext_mod.load_swe_bench_lite(grader="filename")
    assert ds_filename.tasks[0].expected_substring is not None
    assert ds_filename.tasks[0].verify_patch is None

    ds_patch = ext_mod.load_swe_bench_lite(grader="patch")
    assert ds_patch.tasks[0].verify_patch is not None
    assert ds_patch.tasks[0].expected_substring is None
