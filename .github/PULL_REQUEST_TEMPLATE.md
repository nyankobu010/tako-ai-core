<!--
Thanks for contributing to tako!

Before submitting:
- Read CONTRIBUTING.md.
- Sign off your commits (`git commit -s`) — DCO is enforced by CI.
- Use a Conventional Commits prefix (e.g. `feat(tako-core): ...`, `fix(tako-py): ...`, `docs: ...`).
-->

## Summary

<!-- One or two sentences on what this PR changes and why. -->

## Linked issue

<!-- e.g. Closes #123. If there's no issue and the change is non-trivial,
please open one before this PR. -->

## Type

- [ ] feat — new user-visible capability
- [ ] fix — bug fix
- [ ] docs — documentation only
- [ ] refactor — no behaviour change
- [ ] test — test-only change
- [ ] chore — tooling, CI, deps

## Test plan

<!-- Bulleted checklist of how you verified the change. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-features`
- [ ] `ruff check python/ tests/python/ examples/` && `ruff format --check ...`
- [ ] `mypy python/tako`
- [ ] `pytest -q tests/python`
- [ ] (if doc changes) `mkdocs build --strict`

## Checklist

- [ ] Commits are signed off (DCO).
- [ ] One logical concern per commit.
- [ ] Updated `CHANGELOG.md` under `## [Unreleased]` (omit for trivial doc/test-only PRs).
- [ ] Updated `PLAN.md` and `PLAN_PHASE<N>.md` if this PR changes phase scope.
- [ ] Public Rust API additions have rustdoc with at least one runnable example.
- [ ] No new top-level crates without an `ARCHITECTURE.md` update.
