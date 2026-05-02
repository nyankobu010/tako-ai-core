"""Phase 49 — patch-based eval grader.

The eval harness's Phase-1 verifiers (``expected_substring`` /
``expected_regex``) check that the model's text output mentions
the right token. For SWE-Bench-style benchmarks where the model's
output IS a unified diff, that's not real grading.

This module ships :class:`PatchSpec` and :func:`grade_patch` so
operators can build :class:`tako.eval.Task` objects whose
"output" is a patch and whose verifier:

1. Clones a target repo at a specific SHA.
2. Applies the model's patch.
3. Runs an operator-supplied test command.
4. Returns ``(True, _)`` iff the test exits 0.

**Security**. Running model-generated patches + tests is
unsafe with untrusted models. The grader uses plain ``git`` and
the operator's ``test_command`` via ``subprocess`` — there is no
sandbox. Operators wanting isolation must wrap the eval in their
own container. :class:`tako.eval.Eval` defaults to
``allow_unsafe_grader=False`` to surface this requirement
explicitly; passing ``True`` is the operator's acknowledgement
that they've handled the threat model.
"""

from __future__ import annotations

import asyncio
import shutil
import tempfile
from pathlib import Path

from pydantic import BaseModel, Field

__all__ = ["PatchSpec", "grade_patch"]


class PatchSpec(BaseModel):
    """Real-grader spec for a SWE-Bench-style patch task.

    Apply ``model_output`` as a unified diff inside a fresh
    checkout of ``repo`` at ``base_commit``, then run
    ``test_command`` and pass iff exit code is 0.

    Attributes:
        repo: Git URL the grader clones from. Can be ``https://``,
            ``git://``, ``ssh://``, or a ``file://`` path (used in
            tests). Operators using SWE-Bench typically pass
            ``f"https://github.com/{owner}/{name}.git"``.
        base_commit: Commit SHA the patch applies cleanly against.
            ``git checkout`` is run before applying.
        test_command: ``argv``-style list, run with the checkout as
            CWD. e.g. ``["pytest", "-x", "tests/test_foo.py::test_bar"]``.
        apply_timeout_secs: Wall-clock cap on the
            ``git apply`` step. Malformed or massive patches are
            rejected after this. Default 60s.
        test_timeout_secs: Wall-clock cap on the
            ``test_command`` step. Hung tests fail closed after
            this. Default 300s.
    """

    repo: str
    base_commit: str
    test_command: list[str] = Field(default_factory=list)
    apply_timeout_secs: int = 60
    test_timeout_secs: int = 300


async def grade_patch(spec: PatchSpec, model_output: str) -> tuple[bool, str]:
    """Grade ``model_output`` against ``spec``.

    Returns ``(passed, log_excerpt)``. Subprocess errors,
    timeouts, and unparsable patches all surface as
    ``(False, <reason>)`` — no exceptions leak to the caller.

    The function creates a fresh ``tempfile.TemporaryDirectory``
    per call and removes it on every exit path. ``test_command``
    runs with that directory as CWD. ``model_output`` is written
    to ``<tempdir>/.tako_patch.diff`` before ``git apply``.
    """

    if not spec.test_command:
        return False, "PatchSpec.test_command is empty"

    with tempfile.TemporaryDirectory(prefix="tako-grade-") as tmp:
        work = Path(tmp) / "checkout"
        # 1. clone
        clone_rc, clone_log = await _run(
            ["git", "clone", "--quiet", spec.repo, str(work)],
            cwd=tmp,
            timeout=spec.apply_timeout_secs,
        )
        if clone_rc != 0:
            return False, f"git clone failed: {clone_log[-512:]}"

        # 2. checkout base commit
        co_rc, co_log = await _run(
            ["git", "checkout", "--quiet", spec.base_commit],
            cwd=str(work),
            timeout=spec.apply_timeout_secs,
        )
        if co_rc != 0:
            return False, f"git checkout {spec.base_commit} failed: {co_log[-512:]}"

        # 3. write patch + apply
        patch_path = work / ".tako_patch.diff"
        try:
            patch_path.write_text(model_output, encoding="utf-8")
        except OSError as e:
            return False, f"failed to write patch: {e}"

        check_rc, check_log = await _run(
            ["git", "apply", "--check", str(patch_path)],
            cwd=str(work),
            timeout=spec.apply_timeout_secs,
        )
        if check_rc != 0:
            return False, f"git apply --check failed: {check_log[-512:]}"

        apply_rc, apply_log = await _run(
            ["git", "apply", str(patch_path)],
            cwd=str(work),
            timeout=spec.apply_timeout_secs,
        )
        if apply_rc != 0:
            return False, f"git apply failed: {apply_log[-512:]}"

        # 4. run test command
        test_rc, test_log = await _run(
            list(spec.test_command),
            cwd=str(work),
            timeout=spec.test_timeout_secs,
        )
        if test_rc == 0:
            return True, test_log[-512:]
        return False, f"test exit={test_rc}: {test_log[-512:]}"


async def _run(argv: list[str], *, cwd: str, timeout: int) -> tuple[int, str]:
    """Run ``argv`` in ``cwd`` with a wall-clock timeout. Returns
    ``(returncode, combined_stdout_stderr_text)``. Timeouts return
    ``(-1, "<timeout>")`` and best-effort kill the process.

    Errors locating the binary (e.g. ``git`` not on PATH) return
    ``(-2, str(e))`` rather than raising.
    """

    if shutil.which(argv[0]) is None:
        return -2, f"binary not found: {argv[0]!r}"

    proc = await asyncio.create_subprocess_exec(
        *argv,
        cwd=cwd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    try:
        stdout_bytes, _ = await asyncio.wait_for(proc.communicate(), timeout=timeout)
    except asyncio.TimeoutError:
        try:
            proc.kill()
        except ProcessLookupError:
            pass
        return -1, "<timeout>"
    log = (stdout_bytes or b"").decode("utf-8", errors="replace")
    return proc.returncode if proc.returncode is not None else -3, log
