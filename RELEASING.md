# Releasing `tako`

This document covers the release process for `tako` maintainers:
how to cut a version, what CI does on tag push, and the
one-time PyPI Trusted Publisher setup.

## Versioning

`tako` follows [Semantic Versioning](https://semver.org/). While the
project is pre-`1.0`:

- **Minor bumps** (`0.X.0`) for any user-visible change — new
  feature, bug fix, dependency bump.
- **Patch bumps** (`0.X.Y`) reserved for hot-fixes on a previously
  released minor.

The version source-of-truth lives in three places that **must
agree** at release time:

- [`Cargo.toml`](Cargo.toml) — workspace `version` + the 14
  `tako-* = { path = ..., version = "X.Y.Z" }` internal
  dep pins.
- [`pyproject.toml`](pyproject.toml) — `[project] version`.
- [`python/tako/__init__.py`](python/tako/__init__.py) — `__version__`.
- [`tests/python/test_smoke.py`](tests/python/test_smoke.py) —
  `assert tako.__version__ == "X.Y.Z"`. Smoke test fails if a
  bump is missed.

## Cutting a release

1. **Bump the version**. The simplest path is a single commit on
   `main`:

   ```bash
   # Update workspace + internal pins
   sed -i '' 's/version = "0\.51\.0"/version = "0.51.1"/g' Cargo.toml
   sed -i '' 's/version      = "0\.51\.0"/version      = "0.51.1"/g' Cargo.toml
   sed -i '' 's/version = "0\.51\.0"/version = "0.51.1"/g' pyproject.toml
   # Update python facade
   sed -i '' 's/__version__ = "0\.51\.0"/__version__ = "0.51.1"/' python/tako/__init__.py
   sed -i '' 's/"0\.51\.0"/"0.51.1"/g' tests/python/test_smoke.py

   cargo check --workspace   # regenerates Cargo.lock
   ```

   In practice, version bumps land as part of phase-completion PRs;
   see any recent `Phase NN — ...` commit for the pattern.

2. **Stamp the changelog**. Move the `## [Unreleased]` block to
   `## [X.Y.Z] - YYYY-MM-DD` in [`CHANGELOG.md`](CHANGELOG.md).
   Write a one-paragraph summary at the top of the new section
   describing what landed.

3. **Open a PR** with the bump + changelog stamp. CI runs the
   full Rust + Python matrix.

4. **Merge to `main`**.

5. **Tag the release**:

   ```bash
   git checkout main
   git pull origin main
   git tag -a v0.51.1 -m "v0.51.1"
   git push origin v0.51.1
   ```

6. **CI takes over**. The
   [`Build wheels`](.github/workflows/wheels.yml) workflow
   triggers on tag push:

   - **`build` job (matrix)**: Linux x86_64 + aarch64 (gnu +
     musl), macOS universal2, Windows x86_64 + aarch64.
     Each builds with the full release feature set baked
     in (`auth-*`, `sigstore*`, `ws`, `grpc`, `redis`).
   - **`sdist` job**: builds the source distribution.
   - **`publish` job**: downloads all wheels + sdist
     artefacts, publishes to PyPI via the Trusted
     Publisher (no API token in GitHub secrets).

7. **Verify the release** once the workflow finishes:

   ```bash
   # Wait ~1-5 min for PyPI to index, then:
   pip install --upgrade tako-ai-core==0.51.1
   python -c "import tako; print(tako.__version__)"
   ```

## One-time PyPI Trusted Publisher setup

Trusted Publishing lets the GitHub workflow publish to PyPI
without a long-lived API token. It uses OpenID Connect (OIDC)
to mint a short-lived credential per workflow run. **No
secrets in GitHub Actions.**

### Path A: First-time publish (no existing PyPI project)

PyPI has a "Pending Publisher" feature for projects that don't
exist on PyPI yet:

1. Sign in to <https://pypi.org/manage/account/publishing/>.
2. Click **"Add a new pending publisher"**.
3. Fill in:
   - **PyPI Project Name**: `tako-ai-core` (the bare `tako` slot is
     held by an unrelated 2011-era project; `tako-ai-core` matches
     the GitHub repo and is the actual published distribution)
   - **Owner**: `nyankobu010`
   - **Repository name**: `tako-ai-core`
   - **Workflow name**: `wheels.yml`
   - **Environment name**: `pypi`
4. Save.
5. Push your version tag — the first run creates the project
   on PyPI and converts the pending publisher into a regular
   one.

### Path B: Project already on PyPI

If `tako-ai-core` was previously published with a classic API token:

1. Sign in to PyPI as the project owner.
2. Go to <https://pypi.org/manage/project/tako-ai-core/settings/publishing/>.
3. Add a new Trusted Publisher with the same fields as Path A.
4. Optionally remove the old API token under **API tokens**
   once a release has succeeded via Trusted Publishing.

## One-time GitHub setup

The workflow targets two GitHub environments:

- **`pypi`** — required by the `publish` job in
  [`wheels.yml`](.github/workflows/wheels.yml). Acts as the
  approval gate before publishing.
- **`github-pages`** — required by the `deploy` job in
  [`docs.yml`](.github/workflows/docs.yml). Created
  automatically the first time `actions/deploy-pages` runs.

### Create the `pypi` environment

1. Repo settings → **Environments** → **New environment**.
2. Name: `pypi`.
3. Optionally: add **required reviewers** (e.g. yourself)
   so a release blocks on human approval before publishing.
   Recommended for early days; removable later.
4. Optionally: restrict deployment to tag refs matching
   `v*`. The workflow already gates on
   `if: startsWith(github.ref, 'refs/tags/v')`, so this is
   defence-in-depth.

### Enable GitHub Pages

1. Repo settings → **Pages** → **Build and deployment**.
2. Source: **GitHub Actions**.
3. The first push to `main` after this setting is enabled
   triggers [`docs.yml`](.github/workflows/docs.yml), which
   builds + deploys.

## Release checklist

Before tagging, confirm:

- [ ] CI green on `main`.
- [ ] [`CHANGELOG.md`](CHANGELOG.md) has the new section under
      a stamped `## [X.Y.Z] - YYYY-MM-DD` heading.
- [ ] Version is consistent across `Cargo.toml`,
      `pyproject.toml`, `python/tako/__init__.py`, and
      `tests/python/test_smoke.py`.
- [ ] No `## [Unreleased]` block is left with content from
      *this* release (it should be empty / `(none)` after
      the bump).
- [ ] `cargo audit` clean (run in CI; check the latest
      run on `main`).

After tagging:

- [ ] Wheels workflow succeeded (matrix + sdist + publish).
- [ ] `pip install tako-ai-core==X.Y.Z` works from a clean venv;
      `python -c "import tako; print(tako.__version__)"` succeeds.
- [ ] [GitHub Releases](https://github.com/nyankobu010/tako-ai-core/releases) auto-created (if a release-from-tag GitHub action is wired) or manually created with the changelog excerpt.

## Hot-fix releases

For a `0.X.Y` patch on top of an older minor:

1. Branch from the `vX.Y.0` tag: `git checkout -b hotfix-X.Y.Z vX.Y.0`.
2. Cherry-pick or commit the fix.
3. Bump to `X.Y.Z` (patch) following the same pattern.
4. Tag `vX.Y.Z` from the hotfix branch.
5. Open a PR to merge the hotfix branch back into `main`
   (so the fix isn't lost when `main` advances).
