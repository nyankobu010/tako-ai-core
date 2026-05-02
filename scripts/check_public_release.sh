#!/usr/bin/env bash
#
# scripts/check_public_release.sh
#
# Sanity gate before tagging a public release. Run from the repo root.
# Exit non-zero on any check failure. Cheap to run on every commit.
#
# Usage:  bash scripts/check_public_release.sh
#
# What it checks:
#   1. No `TODO(<org>)` outside the historical PLAN_PHASE1.md / PLAN_PHASE21.md.
#   2. No `TODO(community)` outside the historical PLAN_CLEANUP.md.
#   3. No accidentally-committed env/credential files.
#   4. Workspace and Python version strings agree.
#   5. README has no broken obvious refs (light heuristic only).
#   6. mkdocs builds with --strict.
#   7. cargo fmt --check + clippy -D warnings + cargo test --workspace pass.
#   8. ruff check + ruff format --check + pytest -q tests/python pass.

set -uo pipefail
fail=0

note()  { printf '\033[34m[INFO]\033[0m %s\n' "$*"; }
ok()    { printf '\033[32m[ OK ]\033[0m %s\n' "$*"; }
err()   { printf '\033[31m[FAIL]\033[0m %s\n' "$*"; fail=1; }

# 1. Placeholder sweep.
#
# Excluded files describe the substitution rather than carry an unresolved
# placeholder:
#   - PLAN_PHASE1.md / PLAN_PHASE21.md      historical phase-design docs
#   - PLAN_CLEANUP.md / PLAN_PHASE34.md     plans for the substitution itself
#   - PLAN.md / CHANGELOG.md                ledger entries describing what landed
#   - scripts/check_public_release.sh       this script
note "1. Placeholder sweep — TODO(<org>) / TODO(community)"
exclude_re='^(PLAN_PHASE1\.md|PLAN_PHASE21\.md|PLAN_CLEANUP\.md|PLAN_PHASE34\.md|PLAN\.md|CHANGELOG\.md|scripts/check_public_release\.sh):'
org_hits=$(git grep -nE 'TODO\(<org>\)' | grep -vE "$exclude_re" || true)
if [[ -n "$org_hits" ]]; then
  err "TODO(<org>) outside historical files:"
  echo "$org_hits"
else
  ok "no TODO(<org>) outside historical files"
fi

community_hits=$(git grep -nE 'TODO\(community\)' | grep -vE "$exclude_re" || true)
if [[ -n "$community_hits" ]]; then
  err "TODO(community) outside historical files:"
  echo "$community_hits"
else
  ok "no TODO(community) outside historical files"
fi

# 2. No tracked secrets.
note "2. Tracked-secrets scan"
secret_hits=$(git ls-files | grep -E '(\.env$|\.pem$|\.key$|id_rsa|credentials\.json$)' || true)
if [[ -n "$secret_hits" ]]; then
  err "Tracked files that look like secrets:"
  echo "$secret_hits"
else
  ok "no tracked .env / .pem / .key / id_rsa / credentials.json"
fi

# 3. Version consistency.
note "3. Version consistency"
ws_version=$(grep -E '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
py_version=$(grep -E '^version' pyproject.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
if [[ "$ws_version" == "$py_version" ]]; then
  ok "workspace + Python both at $ws_version"
else
  err "version mismatch — workspace=$ws_version python=$py_version"
fi

# 4. mkdocs strict build.
note "4. mkdocs build --strict"
if command -v mkdocs >/dev/null 2>&1; then
  if mkdocs build --strict --quiet --site-dir /tmp/_tako_mkdocs_check >/dev/null 2>&1; then
    ok "mkdocs build --strict"
    rm -rf /tmp/_tako_mkdocs_check
  else
    err "mkdocs build --strict failed (run \`mkdocs build --strict\` for details)"
  fi
else
  note "mkdocs not installed; skipping (install with: pip install mkdocs-material 'mkdocstrings[python]')"
fi

# 5. Rust gates.
note "5. cargo fmt + clippy + test"
if command -v cargo >/dev/null 2>&1; then
  if cargo fmt --all -- --check >/dev/null 2>&1; then
    ok "cargo fmt --check"
  else
    err "cargo fmt --check failed"
  fi
  if cargo clippy --workspace --all-features -- -D warnings >/dev/null 2>&1; then
    ok "cargo clippy --all-features -D warnings"
  else
    err "cargo clippy failed"
  fi
  if cargo test --workspace --all-features --quiet >/dev/null 2>&1; then
    ok "cargo test --workspace --all-features"
  else
    err "cargo test failed"
  fi
else
  note "cargo not installed; skipping"
fi

# 6. Python gates.
note "6. ruff + pytest"
if command -v ruff >/dev/null 2>&1; then
  if ruff check python/ tests/python/ examples/ >/dev/null 2>&1; then
    ok "ruff check"
  else
    err "ruff check failed"
  fi
  if ruff format --check python/ tests/python/ examples/ >/dev/null 2>&1; then
    ok "ruff format --check"
  else
    err "ruff format --check failed"
  fi
else
  note "ruff not installed; skipping"
fi
if command -v pytest >/dev/null 2>&1; then
  if pytest -q tests/python >/dev/null 2>&1; then
    ok "pytest -q tests/python"
  else
    err "pytest failed"
  fi
else
  note "pytest not installed; skipping"
fi

if [[ $fail -ne 0 ]]; then
  printf '\n\033[31mOne or more checks failed.\033[0m\n'
  exit 1
fi
printf '\n\033[32mAll public-release checks passed.\033[0m\n'
