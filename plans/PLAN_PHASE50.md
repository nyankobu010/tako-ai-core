# PLAN — Phase 50 (Open-source release prep + PyPI Trusted Publisher)

> **Status: in progress.** Targets v0.51.0. Repo-housekeeping
> phase. Closes the post-Phase-49 readiness review identified
> after the open-backlog list went empty.

## Context

Phase 49 closed the last open backlog item; the project is
feature-complete for v0.x. A code/repo review surfaced
five OSS-readiness gaps:

1. **`prompt.md` ships in the repo root.** It's the original
   Claude Code planning prompt + full project specification —
   an internal artifact that doesn't belong in a public OSS
   repo. Exposes meta-process; not useful to external readers.
2. **Released wheels are "slim".** [`.github/workflows/wheels.yml`](../.github/workflows/wheels.yml#L36)
   builds with no `--features`. That means `pip install tako`
   from PyPI gives a wheel where `JwtAuth` / `OidcAuth` /
   `VaultAuth` / mTLS rotation / sigstore verification are all
   `None`. Poor first-impression for OSS users.
3. **`docs.yml` builds but doesn't deploy.** The docs site URL
   (`https://nyankobu010.github.io/tako-ai-core`) is
   referenced from [`README.md`](../README.md) and
   [`mkdocs.yml`](../mkdocs.yml) but no workflow publishes to
   GitHub Pages. The `docs.yml` `mkdocs build --strict` step
   only verifies the build doesn't break.
4. **`PLAN.md` intro / phase-discipline section is stale.**
   The "Phase 1 ships the foundation … Do not add Phase 2+
   features" guidance was for early development. After 49
   phases with zero open backlog, the project's posture is
   different: respond to operator asks, fix bugs, evolve.
5. **No `RELEASING.md`.** The release process (tag → wheels.yml
   → PyPI Trusted Publisher) is implemented in CI but
   undocumented. New maintainers can't cut a release without
   reverse-engineering the YAML.

`wheels.yml` itself is **already correctly configured** for
PyPI Trusted Publishing (`id-token: write`,
`environment: pypi`, `pypa/gh-action-pypi-publish@release/v1`,
tag-triggered). The PyPI-side configuration (Trusted
Publisher entry) is the operator's task; this phase
documents the steps so they can do it.

## Why now

- Phase 49 left zero open backlog → natural moment for
  repo housekeeping.
- The user explicitly asked for OSS-readiness review and
  PyPI-release wiring.
- No load-bearing code changes: 100% of this phase is
  metadata, CI, and docs. Risk surface is small.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 50.A | Delete `prompt.md` | [`prompt.md`](../prompt.md) |
| 50.B | Bake auth + sigstore + ws + grpc + redis into released wheels | [`.github/workflows/wheels.yml`](../.github/workflows/wheels.yml) |
| 50.C | Add GitHub Pages deploy step to `docs.yml` | [`.github/workflows/docs.yml`](../.github/workflows/docs.yml) |
| 50.D | New `RELEASING.md` documenting the tag → wheels.yml → PyPI flow + Trusted Publisher setup | [`RELEASING.md`](../RELEASING.md) (new) |
| 50.E | Refresh PLAN.md intro for the post-backlog era | [`PLAN.md`](../PLAN.md) |
| 50.F | "Releasing" section pointer in CONTRIBUTING.md | [`CONTRIBUTING.md`](../CONTRIBUTING.md) |
| 50.G | Workspace + Python version 0.50.0 → 0.51.0 | various |
| 50.H | PLAN.md row + CHANGELOG `[0.51.0]` entry | [`PLAN.md`](../PLAN.md), [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 50.A — Delete `prompt.md`

The file documents Claude Code's planning prompt plus the
full Phase-1 specification. Useful as an internal record
during early development; not appropriate for the public
repo. The actual implementation is what readers care
about, and that's already documented in
[`README.md`](../README.md) /
[`ARCHITECTURE.md`](../ARCHITECTURE.md) /
[`PLAN.md`](../PLAN.md).

`git rm prompt.md` — single deletion. No code references it
(`grep -rn prompt.md` returns only matches inside the file
itself).

### 50.B — Fat wheels for released artefacts

Update the `build` job in
[`.github/workflows/wheels.yml`](../.github/workflows/wheels.yml):

```yaml
- uses: PyO3/maturin-action@v1
  with:
    target: ${{ matrix.target }}
    manylinux: ${{ matrix.manylinux || '' }}
    args: >-
      --release
      --locked
      --compatibility pypi
      --features "auth-jwt auth-oidc auth-vault
        auth-mtls-fs-watch auth-mtls-identity-provider
        sigstore sigstore-protobuf ws grpc redis"
```

Features that **stay opt-in**:

- `onnx` — ONNX Runtime has system-level dynamic-library
  dependencies (`libonnxruntime`); shipping it would
  require bundling the runtime per-platform. Operators
  who need the `OnnxRouter` pyclass build from source.

Features that go **into the released wheel**:

- `auth-jwt` — `jsonwebtoken` (pure Rust).
- `auth-oidc` — JWKS + introspection (pure Rust).
- `auth-vault` — `vaultrs` (pure Rust).
- `auth-mtls-fs-watch` — `notify` (pure Rust).
- `auth-mtls-identity-provider` — `x509-parser` (pure Rust).
- `sigstore` — Sigstore verifier (pure Rust).
- `sigstore-protobuf` — cosign protobuf bundle (pure Rust).
- `ws` — WebSocket MCP transport (pure Rust).
- `grpc` — gRPC MCP transport with mTLS (pure Rust).
- `redis` — Redis budget backend + `RedisStateStore` (pure Rust).

Wheel size grows from ~6 MB to ~20-30 MB compared to slim,
which is acceptable for a framework wheel and matches
typical fat-binary expectations (`pyarrow`, `tensorflow`,
etc., are all 30-200 MB).

### 50.C — Docs deploy

Add a deploy job to
[`.github/workflows/docs.yml`](../.github/workflows/docs.yml)
using the official GitHub Pages actions:

```yaml
deploy:
  name: Deploy to gh-pages
  needs: build
  if: github.ref == 'refs/heads/main'
  runs-on: ubuntu-latest
  permissions:
    pages: write
    id-token: write
  environment:
    name: github-pages
    url: ${{ steps.deployment.outputs.page_url }}
  steps:
    - uses: actions/configure-pages@v5
    - uses: actions/upload-pages-artifact@v3
      with:
        path: ./site
    - id: deployment
      uses: actions/deploy-pages@v4
```

Move the `mkdocs build` output into a separate artefact
that the deploy job downloads. The build job uploads the
`./site` directory as `pages-artifact`; the deploy job
downloads it and pushes to gh-pages.

This requires the GitHub repo's Pages source to be set to
"GitHub Actions" — operator-side config, documented in
`RELEASING.md`.

### 50.D — `RELEASING.md`

New top-level doc covering:

1. **Cutting a release**: bump version, tag, push tag.
2. **What CI does on tag push**: matrix wheel build, sdist,
   PyPI publish via Trusted Publisher.
3. **First-time PyPI Trusted Publisher setup** (operator):
   - Either: register a "Pending Publisher" before first
     publish at https://pypi.org/manage/account/publishing/.
   - Or: do an initial publish with an API token, then
     migrate to Trusted Publisher.
   - Configure GitHub:
     - Owner: `nyankobu010`
     - Repository: `tako-ai-core`
     - Workflow filename: `wheels.yml`
     - Environment name: `pypi`
4. **GitHub environment setup**: ensure the `pypi`
   environment exists in the repo settings; optionally
   add required reviewers for an extra approval step.
5. **Versioning policy**: SemVer. Pre-1.0: minor bumps for
   any user-visible change.
6. **CHANGELOG discipline**: every release has a section
   under `## [Unreleased]` that gets stamped at tag time.
7. **Verifying a release**: `pip install tako==X.Y.Z` from
   PyPI, smoke-import.

### 50.E — PLAN.md intro refresh

Two changes:

1. **Phase index intro** stays largely the same, but the
   "Trait surface in `tako-core` is designed so each phase
   is purely additive — public APIs from earlier phases
   never break" claim should be reinforced as a stability
   commitment now that we're heading toward a public
   release.
2. **"Phase discipline" section** in `CLAUDE.md` (project
   instructions) talks about Phase 1's foundation. Since
   we're past 49 phases, that guidance is no longer
   load-bearing. Leave it as historical context but the
   new posture is "respond to operator asks; never
   regress public API". This stays as-is in CLAUDE.md
   (don't rewrite project instructions in this phase) —
   the PLAN.md intro can carry the new posture.
3. **"Open backlog" section header** explicitly notes
   zero items + invites operator-driven roadmap.

### 50.F — CONTRIBUTING.md pointer

One-paragraph addition pointing at `RELEASING.md`:

```markdown
## Releasing

Maintainers cutting a release should follow [`RELEASING.md`](RELEASING.md)
for the full tag → wheels → PyPI flow, including PyPI
Trusted Publisher configuration.
```

### 50.G — Version bump

0.50.0 → 0.51.0 across `Cargo.toml` (workspace + 14
internal crate version pins), `pyproject.toml`,
`python/tako/__init__.py`, `tests/python/test_smoke.py`.

### 50.H — PLAN.md row + CHANGELOG

Standard.

## Critical files

**Modified:**
- [`.github/workflows/wheels.yml`](../.github/workflows/wheels.yml) (50.B).
- [`.github/workflows/docs.yml`](../.github/workflows/docs.yml) (50.C).
- [`PLAN.md`](../PLAN.md) (50.E + 50.H).
- [`CONTRIBUTING.md`](../CONTRIBUTING.md) (50.F).
- Standard PLAN/CHANGELOG/version flip:
  [`Cargo.toml`](../Cargo.toml), [`pyproject.toml`](../pyproject.toml),
  [`python/tako/__init__.py`](../python/tako/__init__.py),
  [`tests/python/test_smoke.py`](../tests/python/test_smoke.py),
  [`CHANGELOG.md`](../CHANGELOG.md).

**Deleted:**
- [`prompt.md`](../prompt.md) (50.A).

**Created:**
- [`RELEASING.md`](../RELEASING.md) (50.D).
- [`plans/PLAN_PHASE50.md`](PLAN_PHASE50.md) (this file).

## Verification

1. `cargo fmt --all -- --check` (no Rust changes; sanity).
2. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
3. `cargo test --workspace --exclude tako-py --all-features` — no regressions.
4. `ruff format --check` + `ruff check`.
5. Static `actionlint` check on the YAML files (best-effort
   — not a CI gate, but run locally if available).
6. `mkdocs build --strict` — docs build cleanly.
7. `maturin develop --release --features "auth-jwt auth-oidc auth-vault auth-mtls-fs-watch auth-mtls-identity-provider sigstore sigstore-protobuf ws grpc redis"` — wheel builds with the full release feature set at v0.51.0.
8. `pytest -q` — full suite green; smoke pins v0.51.0.

## Out of scope

- **Updating GitHub repo settings** (Pages source, branch
  protection, environment reviewers). Operator concern;
  documented in `RELEASING.md`.
- **Bundling ONNX Runtime in the wheel.** Significant
  per-platform binary management; defer until ask.
- **Splitting the wheel** into `tako-core` + `tako-auth-*`
  PyPI distributions. Cargo features in one wheel are
  simpler to maintain than multiple PyPI packages.
- **CHANGELOG release-tag automation.** A
  `release-please`-style tool could sync `Unreleased` →
  `[X.Y.Z]` on tag push. Worth doing later; for now the
  release process is documented in `RELEASING.md` and
  done manually.
- **Renaming the project / repo.** `nyankobu010/tako-ai-core`
  is set in `mkdocs.yml`, `Cargo.toml`'s repository field,
  the README badges, etc. Out of scope for this phase.
- **`CLAUDE.md` rewrite.** Keep it as-is; it's project
  instructions, not user-facing.
