# PLAN — Cleanup (public-release prep)

> **Status: not started.** Out-of-band from the version-tagged feature
> phases ([PLAN_PHASE10.md](PLAN_PHASE10.md) and earlier); does not bump
> the workspace version. Lands as one focused PR. Closes the
> *First-publish placeholders* backlog in [PLAN.md](PLAN.md#L115-L127)
> and the *Documentation maintenance* item at
> [PLAN.md:108-113](PLAN.md#L108-L113).
>
> **Naming:** intentionally not `PLAN_PHASE<N>.md` — this is a release-prep
> sweep, not a feature phase. The next feature phase remains
> [PLAN_PHASE10.md](PLAN_PHASE10.md) at v0.11.0.

## Context

`tako-ai-core` has been developed against the literal placeholder string
`TODO(<org>)` so the eventual public-org name was a single grep-able point
of edit (per [PLAN_PHASE1.md:55](PLAN_PHASE1.md#L55)). The project is
otherwise public-ready as of v0.10.0 (2026-04-30):

- Apache-2.0 licensed; comprehensive [NOTICE](NOTICE) with TreeQuest +
  Sakana paper attribution and dependency licenses listed.
- No secrets, internal references, or vendored third-party code.
- Tests run without API keys (in-process `FakeProvider`).
- 17 runnable example scripts under [examples/](examples/) with FakeProvider
  fallback.
- CI uses PyPI Trusted Publishing — no long-lived `PYPI_TOKEN`.
- `.gitignore` is comprehensive; no tracked binaries.

What blocks the actual public switch is mechanical placeholder substitution
plus a short list of OSS hygiene files [CONTRIBUTING.md](CONTRIBUTING.md)
already promises but doesn't deliver (e.g. DCO sign-off is mandated at
[CONTRIBUTING.md:41](CONTRIBUTING.md#L41) but not enforced by any CI job).

**Inputs (resolved):**

- GitHub owner: `nyankobu010` → repo `https://github.com/nyankobu010/tako-ai-core`.
- Contact-email strategy: GitHub *Private Vulnerability Reporting* for SECURITY.md
  (no email needed); maintainer noreply `9086161+nyankobu010@users.noreply.github.com`
  for CODE_OF_CONDUCT.md.

## Scope summary

| Section | What | Files touched |
|---------|------|---------------|
| C.A | `TODO(<org>)` URL substitution (22 sites) | 11 files |
| C.B | `TODO(community)` contact substitution (4 sites) | 4 files |
| C.C | OSS hygiene file additions | 5 new files |
| C.D | Polish (model name, index.md sync, sanity script, link audit) | 2 modified + 1 new |
| C.E | Index updates (PLAN.md, CHANGELOG.md `## [Unreleased]`) | 2 files |

## What this phase will land

### C.A — `TODO(<org>)` URL substitution

`git grep -nE 'TODO\(<org>\)'` enumerates every site. Substitute literal
`TODO(<org>)` → `nyankobu010` across these *non-self-referential* hits:

- [Cargo.toml:26](Cargo.toml#L26) — workspace `repository` URL.
- [pyproject.toml:39-42](pyproject.toml#L39-L42) — `Homepage`, `Documentation`,
  `Repository`, `Issues` (4 hits).
- [mkdocs.yml:3-5](mkdocs.yml#L3-L5) — `site_url`, `repo_url`, `repo_name`
  (3 hits).
- [README.md:7](README.md#L7) — CI badge URL + link target (2 hits in one
  line); [README.md:156](README.md#L156) — Issues URL;
  [README.md:159](README.md#L159) — good-first-issues URL.
- [CONTRIBUTING.md:19](CONTRIBUTING.md#L19) — clone URL.
- [CHANGELOG.md:1157-1167](CHANGELOG.md#L1157-L1167) — 11 version-compare
  links (`[Unreleased]:` + 10 per-version anchors).
- [crates/tako-core/src/lib.rs:8-9](crates/tako-core/src/lib.rs#L8-L9) —
  rustdoc cross-references to README + ARCHITECTURE.
- [docs/concepts/policy.md:69](docs/concepts/policy.md#L69) — example
  policy file link.

**Self-referential hits to LEAVE in place** (these explain why the
placeholder existed and are closed by this very phase):

- [PLAN.md:117, :123](PLAN.md#L117) — backlog items; will be deleted
  by C.E (not substituted).
- [PLAN_PHASE1.md:55](PLAN_PHASE1.md#L55) — historical design rationale
  ("literal string `TODO(<org>)` — grep-able single point of edit"). Keep
  for historical accuracy of the Phase 1 plan.

**Verification:** after substitution,
`git grep -nE 'TODO\(<org>\)'` returns only the two hits in
`PLAN_PHASE1.md:55` (historical rationale) — zero hits in any other file.

### C.B — `TODO(community)` contact substitution

- [README.md:157](README.md#L157) — `Discussions: TODO(community): set up GitHub Discussions categories Q&A / Ideas / Show and tell.`
  → either enable Discussions and link
  `https://github.com/nyankobu010/tako-ai-core/discussions`, or remove the
  bullet entirely (Issues remains the primary channel).
- [README.md:158](README.md#L158) — `Chat: TODO(community): create a Discord/Matrix room and link here.`
  → remove the bullet (no chat channel planned for initial release; can be
  re-added when one exists).
- [CODE_OF_CONDUCT.md:3](CODE_OF_CONDUCT.md#L3) — `Report violations to TODO(community): conduct@<placeholder>`
  → `Report violations to the maintainer at 9086161+nyankobu010@users.noreply.github.com`.
- [SECURITY.md:7](SECURITY.md#L7) — `or email TODO(community): security@<placeholder>`
  → replace email line with: `Use this repository's *Private Vulnerability
  Reporting* (Security tab on GitHub).` Then enable PVR in repo settings
  (see C.F).

### C.C — OSS hygiene additions

Five new files. None are blockers, but together they close the gap
between what [CONTRIBUTING.md](CONTRIBUTING.md) promises and what the
repo actually enforces.

#### C.C.1 — `.github/PULL_REQUEST_TEMPLATE.md`

Mirrors the requirements already in CONTRIBUTING.md: linked issue,
Conventional Commit type, DCO sign-off checkbox, test-plan checklist,
note about updating `CHANGELOG.md` `## [Unreleased]`.

#### C.C.2 — `.github/CODEOWNERS`

Single line: `* @nyankobu010`. Auto-requests review on every PR; trivially
extensible later when more maintainers join.

#### C.C.3 — `SUPPORT.md`

Three sections — Bugs (point to Issues), Questions (Discussions if
enabled, else Issues), Vulnerabilities (point to SECURITY.md). Removes
ambiguity for first-time contributors.

#### C.C.4 — `.github/workflows/dco.yml`

DCO check workflow using `tim-actions/dco@v1.1.0` (or equivalent maintained
action). Triggers on `pull_request: [opened, synchronize, reopened]`.
Without this, the `git commit -s` requirement at
[CONTRIBUTING.md:41](CONTRIBUTING.md#L41) is honour-system only.

```yaml
# Sketch — finalize during execution.
name: DCO
on:
  pull_request:
    types: [opened, synchronize, reopened]
jobs:
  dco:
    runs-on: ubuntu-latest
    steps:
      - uses: tim-actions/dco@v1.1.0
```

#### C.C.5 — `.github/FUNDING.yml` (optional, default omit)

Skip unless GitHub Sponsors is enabled before release. If included, single
field: `github: [nyankobu010]`. Decision deferred to execution time.

### C.D — Polish

#### C.D.1 — README quickstart model verification

[README.md:62](README.md#L62) references
`tako.providers.OpenAI(model="gpt-5")`. Verify against OpenAI's published
model catalog at the time of execution; replace with the current GA model
name if `gpt-5` is no longer accurate. Independently `python -c` the
example with `OPENAI_API_KEY` unset to confirm the FakeProvider fallback
path triggers cleanly.

#### C.D.2 — `docs/index.md` feature matrix sync

The audit flagged [docs/index.md](docs/index.md) as still referencing
Phase 2.5 / v0.3.0 while [README.md](README.md) is current to Phase 9.
Bring index.md to parity by mirroring the README feature-matrix and
roadmap sections. Verify with `mkdocs build -f docs/mkdocs.yml --strict`.

#### C.D.3 — `scripts/check_public_release.sh`

New helper script. Cheap to keep around for future contributors. Checks:

```bash
# 1. No TODO(<org>) outside PLAN_PHASE1.md (historical rationale).
# 2. No accidentally-committed *.env / *.key / *.pem / id_rsa* files.
# 3. Workspace + Python version strings match.
# 4. README links are well-formed (no obvious 404s — local check only).
# 5. mkdocs builds with --strict.
# 6. cargo fmt --check && cargo clippy -- -D warnings && cargo test --workspace pass.
# 7. ruff check && pytest -q pass.
```

Wire into CI later if useful; for now a documented manual gate.

#### C.D.4 — Link / badge audit

After C.A substitution, click-through every badge/link in:
[README.md](README.md), [CONTRIBUTING.md](CONTRIBUTING.md),
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md), [SECURITY.md](SECURITY.md),
[SUPPORT.md](SUPPORT.md), [docs/index.md](docs/index.md). The CI badge
will 404 until the workflow has run on `main` post-rename — note this as
expected and not a blocker.

#### C.D.5 — NOTICE year confirmation

[NOTICE](NOTICE) currently reads `Copyright 2026 The tako contributors`.
Confirm 2026 is intentional (project genesis year, not a mistake from a
forward-dated dev environment). No change expected; this is a sanity
check.

### C.E — Index updates

- [PLAN.md](PLAN.md): delete the "First-publish placeholders" backlog
  section ([PLAN.md:115-127](PLAN.md#L115-L127)) and the README feature
  matrix item ([PLAN.md:108-113](PLAN.md#L108-L113), now resolved by
  README being current and C.D.2 syncing index.md). Add one row to the
  phase index table referencing this plan, e.g.:

  ```
  | Cleanup — public-release prep | (no version bump) | done (YYYY-MM-DD) | PLAN_CLEANUP.md | [`## [Unreleased]`](CHANGELOG.md) |
  ```

- [CHANGELOG.md](CHANGELOG.md): under `## [Unreleased]`, add a `### Changed`
  entry — *"Replaced `TODO(<org>)` placeholders with the public GitHub
  org `nyankobu010`. Added DCO enforcement workflow, PR template,
  CODEOWNERS, SUPPORT.md. Synced docs/index.md feature matrix to
  Phase 9."*

### C.F — Repo settings (out-of-tree, GitHub web UI)

Not in-repo work, but listed here so it isn't forgotten:

- [ ] Rename / create repo at `github.com/nyankobu010/tako-ai-core`.
- [ ] Enable Issues (default).
- [ ] Enable Discussions (if linking from README per C.B).
- [ ] Enable *Private Vulnerability Reporting* (Settings → Code security
      and analysis). Required for SECURITY.md to be honest after C.B.
- [ ] Default branch: `main` (already is in local repo).
- [ ] Repo description (≤350 chars): `Rust-core, Python-facade framework
      for enterprise agentic systems. Many arms, one mind.`
- [ ] Topics: `rust`, `python`, `pyo3`, `agents`, `llm`, `mcp`,
      `apache-2`, `opentelemetry`, `mcts`.
- [ ] PyPI Trusted Publisher: configure on PyPI side to match
      `wheels.yml` GitHub OIDC subject.
- [ ] (optional) GitHub Sponsors → `.github/FUNDING.yml` per C.C.5.

## Critical files

**Modified:**

- [Cargo.toml](Cargo.toml), [pyproject.toml](pyproject.toml),
  [mkdocs.yml](mkdocs.yml) — manifests / docs site config.
- [README.md](README.md) — badges, community section, possibly quickstart
  model name.
- [CONTRIBUTING.md](CONTRIBUTING.md) — clone URL.
- [CHANGELOG.md](CHANGELOG.md) — version compare anchors + new
  `## [Unreleased]` entry.
- [SECURITY.md](SECURITY.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) —
  contact methods.
- [crates/tako-core/src/lib.rs](crates/tako-core/src/lib.rs) — rustdoc
  cross-references.
- [docs/concepts/policy.md](docs/concepts/policy.md) — example link.
- [docs/index.md](docs/index.md) — feature matrix sync.
- [PLAN.md](PLAN.md) — index update + backlog cleanup.

**Created:**

- [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md)
- [.github/CODEOWNERS](.github/CODEOWNERS)
- [.github/workflows/dco.yml](.github/workflows/dco.yml)
- [SUPPORT.md](SUPPORT.md)
- [scripts/check_public_release.sh](scripts/check_public_release.sh)
- [.github/FUNDING.yml](.github/FUNDING.yml) — optional; default omit.

**Read-only context:**

- [NOTICE](NOTICE) — verify 2026 year (no edit expected).
- [PLAN_PHASE1.md](PLAN_PHASE1.md) — self-referential `TODO(<org>)` at
  line 55 stays as historical rationale.

## Verification

End-to-end checks the cleanup phase must pass:

1. **Placeholder sweep clean.** `git grep -nE 'TODO\(<org>\)'` returns
   only [PLAN_PHASE1.md:55](PLAN_PHASE1.md#L55).
   `git grep -nE 'TODO\(community\)'` returns zero hits.
2. **Manifest version consistency.** Workspace and Python versions still
   agree:

   ```bash
   grep -E '^version' Cargo.toml
   grep -E '^version' pyproject.toml
   ```

3. **Build green.** Per [CLAUDE.md](CLAUDE.md):

   ```bash
   cargo fmt --all
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   maturin develop --release
   pytest -q
   ```

4. **Mkdocs renders strict.** `mkdocs build -f docs/mkdocs.yml --strict`
   passes. Catches any cross-link the manual audit missed.
5. **DCO workflow gates a PR.** Open a throwaway PR with one unsigned
   commit; confirm the new `dco.yml` job fails. Append `--amend -s` (or a
   new signed commit), confirm it passes.
6. **Sanity script clean.** `bash scripts/check_public_release.sh`
   exits 0.
7. **Smoke an example.** `python examples/01_quickstart.py` runs to
   completion with no API keys set (FakeProvider fallback path).
8. **External clone test (highest signal).** In `/tmp`:

   ```bash
   git clone https://github.com/nyankobu010/tako-ai-core
   cd tako-ai-core
   uv venv .venv && source .venv/bin/activate
   uv pip install -e '.[dev]'
   maturin develop --release
   pytest -q
   ```

   This is exactly what a stranger does. If it works, the project is
   publicly usable.

## Out of scope

Listing explicitly so creep stays out:

- **No code changes** beyond the README quickstart model name (C.D.1) and
  the rustdoc URL fixes in `tako-core/src/lib.rs` (C.A). No provider,
  orchestrator, or test changes.
- **No version bump.** Cleanup ships under `## [Unreleased]`; the next
  version bump comes with [PLAN_PHASE10.md](PLAN_PHASE10.md) (v0.11.0).
- **No NOTICE rewrite.** It's already comprehensive; only the 2026 year
  is sanity-checked.
- **No license change.** Apache-2.0 stays.
- **No DCO history rewrite.** The new workflow enforces forward-only;
  pre-cleanup commits are not retroactively signed.
- **No new features from the [PLAN.md backlog](PLAN.md#L68-L104).** Vision
  support, eval graders, MCP SSE upgrade, and friends remain future-phase
  work.
