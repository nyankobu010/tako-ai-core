# PLAN — Phase 49 (Eval harness patch grader)

> **Status: in progress.** Targets v0.50.0. Closes the
> "Eval harness real graders" carry-forward — the **last
> open item** in [PLAN.md](../PLAN.md)'s backlog.

## Context

Phase 4 (v0.5.0) shipped the eval harness with two
verifier modes:

- `Task.expected_substring` — output must contain a literal
  string.
- `Task.expected_regex` — output must match a regex.

For SWE-Bench (`princeton-nlp/SWE-bench_Lite`), the
[current adapter](../python/tako/eval/datasets/external.py#L66-L91)
extracts the first filename from the gold patch's
`--- a/<path>` header and uses it as a substring verifier.
Functional but not real grading: a model answer that just
mentions `query.py` in prose passes; a model answer that
emits a correct patch but uses different words for the
filename in prose can fail. The PLAN.md backlog has
flagged this as deferred since Phase 4:

> **Eval harness real graders.** Real SWE-Bench (apply
> patch + run sandboxed repo tests) deferred to "a later
> phase".

Phase 49 ships a real **patch grader**: an opt-in verifier
that takes the model's output as a unified diff, applies
it inside a temp-dir checkout of a target repo at a
specific SHA, runs an operator-supplied test command, and
passes iff the test exit code is 0.

## Why now

After Phase 48 the only open backlog item is this one.
Closing it lands the project on zero open backlog. The
fix is bounded in scope:

- New `PatchSpec` Pydantic model holding the repo URL,
  base commit, test command, and timeouts.
- New `Task.verify_patch: PatchSpec | None` field.
- New async `Task.verify_async(output)` method that
  delegates to the existing sync `passes()` for
  substring/regex tasks and to a subprocess-based
  patch runner for patch tasks.
- New SWE-Bench adapter mode (`grader="patch"`) that
  builds patch tasks from dataset rows.
- A security gate (`Eval.allow_unsafe_grader=False` by
  default) — running model-generated code is dangerous;
  operators must opt in explicitly.

No Docker dependency. The grader uses plain `git` + the
operator's pytest (or whatever test command they
configure) via `subprocess`. **Operators wanting
true sandboxing wrap the eval in their own container**
— Phase 49 is the substrate; container hosting is an
operator concern.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 49.A | New `PatchSpec` model + `verify_patch` async runner module | [`python/tako/eval/grader.py`](../python/tako/eval/grader.py) (new) |
| 49.B | Extend `Task` with `verify_patch: PatchSpec | None` + async `verify_async()` | [`python/tako/eval/harness.py`](../python/tako/eval/harness.py) |
| 49.C | `Eval.allow_unsafe_grader: bool = False` security gate; harness uses `verify_async` | [`python/tako/eval/harness.py`](../python/tako/eval/harness.py) |
| 49.D | SWE-Bench adapter `grader="patch"` mode + `_swe_bench_to_patch_task` | [`python/tako/eval/datasets/external.py`](../python/tako/eval/datasets/external.py) |
| 49.E | Tests: PatchSpec, mini-repo end-to-end grader, SWE-Bench adapter, security gate | [`tests/python/test_phase49_patch_grader.py`](../tests/python/test_phase49_patch_grader.py) (new) |
| 49.F | Docstring updates removing "deferred to a later phase" caveats | [`python/tako/eval/harness.py`](../python/tako/eval/harness.py), [`python/tako/eval/datasets/external.py`](../python/tako/eval/datasets/external.py) |
| 49.G | Workspace + Python version 0.49.0 → 0.50.0 | various |
| 49.H | PLAN.md row + close last backlog item | [`PLAN.md`](../PLAN.md) |
| 49.I | CHANGELOG.md `[0.50.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 49.A — `PatchSpec` model + async runner

`python/tako/eval/grader.py`:

```python
class PatchSpec(BaseModel):
    """Real-grader spec for a SWE-Bench-style patch task.

    Apply ``model_output`` as a unified diff inside a fresh
    checkout of ``repo`` at ``base_commit``, then run
    ``test_command`` and pass iff exit code is 0.

    All paths inside the checkout are operator-controlled;
    no data leaves the temp dir.
    """

    repo: str  # e.g. "https://github.com/django/django.git"
    base_commit: str  # SHA the patch applies cleanly to
    test_command: list[str]  # e.g. ["pytest", "-x", "tests/test_x.py::test_y"]
    apply_timeout_secs: int = 60
    test_timeout_secs: int = 300
    work_dir: str | None = None  # if set, use as parent for the checkout

async def grade_patch(
    spec: PatchSpec, model_output: str
) -> tuple[bool, str]:
    """Returns (passed, log_excerpt). Subprocess errors and
    timeouts surface as `(False, <reason>)` — no exceptions
    leak to the caller."""
```

Implementation: an `asyncio.create_subprocess_exec` chain
that:

1. `git clone --no-checkout <repo> <tempdir>` (shallow
   when SHA is fetchable, fall back to full clone).
2. `git -C <tempdir> checkout <base_commit>`.
3. Write `model_output` to `<tempdir>/.tako_patch.diff`.
4. `git -C <tempdir> apply --check .tako_patch.diff` —
   reject malformed patches early.
5. `git -C <tempdir> apply .tako_patch.diff`.
6. Run `test_command` inside `<tempdir>` with a hard
   `test_timeout_secs` wall-clock cap.
7. Parse exit code; on success, return `(True, <stdout
   tail>)`. Otherwise `(False, <stderr tail>)`.

Any subprocess failure (clone error, malformed patch,
timeout) returns `(False, <reason>)` — never raises.
Cleanup: temp dir removed on every exit path via
`tempfile.TemporaryDirectory()`.

### 49.B — `Task.verify_patch` + `verify_async`

```python
class Task(BaseModel):
    id: str
    prompt: str
    expected_substring: str | None = None
    expected_regex: str | None = None
    verify_patch: PatchSpec | None = None  # NEW
    max_tokens: int | None = None

    def passes(self, output: str) -> bool:
        # Unchanged — substring/regex only. Patch tasks
        # without `expected_*` always return False here so
        # callers that haven't migrated to verify_async
        # see a clear "fail".
        ...

    async def verify_async(self, output: str) -> bool:
        if self.verify_patch is not None:
            ok, _log = await grade_patch(self.verify_patch, output)
            return ok
        return self.passes(output)
```

Backwards-compatible:
- Tasks with only substring/regex unchanged.
- The new `verify_patch` field is optional.
- The sync `passes()` API still works.

### 49.C — Security gate

```python
class Eval(BaseModel):
    ...
    allow_unsafe_grader: bool = False  # NEW

    async def run(self) -> EvalReport:
        # Validate: any patch task requires explicit opt-in.
        has_patch = any(t.verify_patch is not None for t in self.dataset.tasks)
        if has_patch and not self.allow_unsafe_grader:
            raise ValueError(
                "Patch grading runs model-generated code via subprocess, "
                "which is unsafe with untrusted models. Pass "
                "`Eval(allow_unsafe_grader=True)` to enable, and "
                "consider running the eval inside a container."
            )
        ...
```

The harness's `run_attempt` now calls `await task.verify_async(text)`
instead of `task.passes(text)`.

### 49.D — SWE-Bench `grader="patch"` mode

```python
def load_swe_bench_lite(
    split: str = "test",
    *,
    limit: int | None = None,
    grader: Literal["filename", "patch"] = "filename",
) -> Dataset:
    ...

def _swe_bench_to_patch_task(row: dict[str, Any]) -> Task:
    """Builds a patch-graded Task from a SWE-Bench row."""
    instance_id = str(row["instance_id"])
    repo = str(row["repo"])  # e.g. "django/django"
    base = str(row["base_commit"])
    fail_to_pass = row.get("FAIL_TO_PASS") or "[]"
    # FAIL_TO_PASS is a JSON-encoded list of pytest test ids.
    if isinstance(fail_to_pass, str):
        fail_to_pass = json.loads(fail_to_pass)
    spec = PatchSpec(
        repo=f"https://github.com/{repo}.git",
        base_commit=base,
        test_command=["pytest", "-x", *fail_to_pass[:5]],
        apply_timeout_secs=60,
        test_timeout_secs=600,
    )
    return Task(
        id=instance_id,
        prompt=_swe_bench_prompt(row),
        verify_patch=spec,
    )
```

`grader="filename"` keeps the Phase 4 behaviour (default
for back-compat). `grader="patch"` is the opt-in real
grader.

### 49.E — Tests

`tests/python/test_phase49_patch_grader.py`:

1. **`PatchSpec` validation** — required fields, defaults
   for timeouts.
2. **`grade_patch` happy path** — uses `pytest` and `git`
   from `PATH`. Test fixture:
   - Initialises an empty `git` repo in `tmp_path`.
   - Adds `module.py` with a deliberately failing
     `test_x.py` (e.g. `assert add(2, 3) == 5` against an
     incorrect impl).
   - Commits to `main`.
   - Builds a `PatchSpec` pointing at the local repo
     (via `file://`).
   - Hand-crafts a unified diff that fixes the impl.
   - Asserts `grade_patch(spec, diff)` returns `(True, _)`.
3. **`grade_patch` rejects a non-applying diff** — patch
   targets a file that doesn't exist.
4. **`grade_patch` rejects a patch that applies cleanly
   but the test still fails**.
5. **`grade_patch` enforces the apply timeout** —
   `apply_timeout_secs=0` always fails fast.
6. **`Task.verify_async` dispatches correctly** — patch
   path vs. substring path.
7. **`Eval` raises without `allow_unsafe_grader`** when
   the dataset contains a patch task.
8. **`Eval` runs end-to-end** with `allow_unsafe_grader=True`
   against the mini-repo fixture.
9. **`_swe_bench_to_patch_task`** produces a sane
   `PatchSpec` from a mock row (asserts `repo` URL is
   built correctly, `test_command` first arg is
   `pytest`, `base_commit` matches).

The mini-repo fixture (#2) requires `git` on PATH (CI
already provides it). `pytest` is already a dev-dep. No
network needed because we use `file://` URIs.

### 49.F — Docstring updates

Remove the "deferred to a later phase" claim from
[harness.py](../python/tako/eval/harness.py) and
[external.py](../python/tako/eval/datasets/external.py).
Replace with accurate prose that points at
`grader="patch"` and the security model.

### 49.G — Version bump

0.49.0 → 0.50.0. Workspace `Cargo.toml` (workspace + 14
internal crate version pins), `pyproject.toml`,
`python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 49.H — PLAN.md

- New row `49 — Eval harness patch grader`.
- Flip the "Eval harness real graders" backlog item to
  closed-by-Phase-49.
- Note in the PLAN preamble that the open-backlog list
  is now **empty** (post-Phase-49).

### 49.I — CHANGELOG `[0.50.0]`

The 0.50.0 round-number deserves prose acknowledging this
closes the last backlog item. Otherwise standard format.

## Critical files

**Modified:**
- [`python/tako/eval/harness.py`](../python/tako/eval/harness.py) — `Task.verify_patch`, `Task.verify_async`, `Eval.allow_unsafe_grader`.
- [`python/tako/eval/datasets/external.py`](../python/tako/eval/datasets/external.py) — `grader="patch"` parameter + `_swe_bench_to_patch_task`.
- Standard PLAN/CHANGELOG/version flip.

**Created:**
- [`python/tako/eval/grader.py`](../python/tako/eval/grader.py) — `PatchSpec` + `grade_patch` runner.
- [`tests/python/test_phase49_patch_grader.py`](../tests/python/test_phase49_patch_grader.py).
- [`plans/PLAN_PHASE49.md`](PLAN_PHASE49.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
3. `cargo test --workspace --exclude tako-py --all-features` — no regressions (no Rust changes).
4. `ruff format --check` + `ruff check`.
5. `mypy python/tako/eval` (if configured).
6. `pytest -q tests/python/test_phase49_patch_grader.py` — new tests pass.
7. `pytest -q` — full suite green; smoke pins v0.50.0.
8. `maturin develop --release` — wheel builds at v0.50.0.

## Out of scope

- **Docker / sandboxed container runner.** Operators
  wrap the eval in their own container. Adding a Docker
  runner here would couple tako to a specific runtime;
  better as a recipe than a built-in.
- **`PASS_TO_PASS` verification.** SWE-Bench rows include
  both `FAIL_TO_PASS` (tests that should now pass) and
  `PASS_TO_PASS` (tests that should keep passing). Phase
  49 only checks `FAIL_TO_PASS` — the harder, more
  important contract. PASS_TO_PASS is a follow-up.
- **Test runners other than pytest.** SWE-Bench instances
  vary (unittest, tox, nose, etc.). Phase 49 hard-codes
  pytest in the SWE-Bench adapter. Operators with
  custom datasets supply their own `test_command` so
  this isn't a blanket limitation; it's just that
  SWE-Bench loader assumes pytest.
- **Caching the cloned repo.** Each task currently
  clones fresh. For a 300-task SWE-Bench-Lite run that
  means 300 clones — slow, but not incorrect.
  Per-`(repo, sha)` shallow-clone cache is a recipe-
  level concern; defer until ask.
- **Grader for `gpqa_diamond`.** GPQA's existing A/B/C/D
  positional verifier is fine; the dataset is multiple-
  choice and "real grading" doesn't add anything.
