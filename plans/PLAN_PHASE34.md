# PLAN — Phase 34: Public-release prep, tech-debt + docs sweep

> **Status: in progress.** Targets v0.35.0. Original Phase 34 candidates
> (trait-based `MtlsIdentityProvider`, automatic refresh-on-handshake-
> failure, filesystem-watcher integration, etc.) are postponed to
> Phase 35+ — see [PLAN.md](PLAN.md#phase-35-candidates-indicative-not-yet-committed).

## Context

`tako-ai-core` has shipped 33 feature phases (Phase 1 through Phase 33
= v0.1.0 through v0.34.0) and is functionally ready for public release.
What blocks the public switch is mechanical hygiene work the project
deferred all along under [`PLAN_CLEANUP.md`](PLAN_CLEANUP.md), plus a
documentation refresh — `docs/index.md` was still pinned to v0.3.0 /
Phase 2.5, the mkdocs nav was missing concept pages for nine phases'
worth of features, and `CHANGELOG.md` compare-link anchors stopped at
v0.14.0.

Phase 34 supersedes [`PLAN_CLEANUP.md`](PLAN_CLEANUP.md) (which was
written at the v0.10.0 stage and never executed). The cleanup plan's
scope is folded into this phase, then extended to cover the docs work
that accumulated during Phases 11–33.

## Scope summary

| Section | What | Files touched |
|---------|------|---------------|
| 34.A | Placeholder substitution (`TODO(<org>)` → `nyankobu010`) | 11 files (Cargo.toml, pyproject.toml, mkdocs.yml, README.md, CONTRIBUTING.md, CHANGELOG.md, lib.rs rustdoc, docs/concepts/policy.md) |
| 34.B | `TODO(community)` substitution (4 sites) | README.md, CODE_OF_CONDUCT.md, SECURITY.md |
| 34.C | OSS hygiene files | `.github/PULL_REQUEST_TEMPLATE.md`, `.github/CODEOWNERS`, `.github/workflows/dco.yml`, `SUPPORT.md`, `CITATION.cff` |
| 34.D | Documentation refresh | docs/index.md (Phase 2.5 → Phase 34), docs/architecture.md, docs/quickstart.md, concepts/{providers,orchestrators,budgets,mcp,secrets}.md |
| 34.E | New documentation pages | concepts/{vision,url_prefetch,streaming,compat,sigstore}.md; recipes/{mistral,ollama,vision,url_prefetch,oidc_introspection,chained_auth,mtls_rotation,sigstore_keyless}.md |
| 34.F | Mkdocs nav update | mkdocs.yml |
| 34.G | Sanity script | scripts/check_public_release.sh |
| 34.H | CHANGELOG anchors + Phase 34 entry | CHANGELOG.md |
| 34.I | Workspace + Python version bump 0.34.0 → 0.35.0 | Cargo.toml, pyproject.toml, _native.pyi |
| 34.J | PLAN.md index + roadmap update | PLAN.md |

## What this phase will land

### 34.A — `TODO(<org>)` substitution

GitHub owner: `nyankobu010`. Substitute literal `TODO(<org>)` →
`nyankobu010` across all non-self-referential sites. Self-referential
sites (PLAN_PHASE1.md:55 historical rationale, PLAN_PHASE21.md:239
historical phase doc, PLAN_CLEANUP.md as the *source* document
describing the placeholder strategy) are intentionally left alone.

After 34.A, `git grep -nE 'TODO\(<org>\)'` returns only the historical
hits — verified by [`scripts/check_public_release.sh`](scripts/check_public_release.sh).

### 34.B — `TODO(community)` substitution

- README.md community section: replace the `Discussions: TODO(community): ...`
  / `Chat: TODO(community): ...` lines with the GitHub Discussions URL
  and a SECURITY.md pointer. No chat channel for v0.35.0.
- CODE_OF_CONDUCT.md: replace the `conduct@<placeholder>` line with the
  maintainer's GitHub noreply address + GitHub Private Vulnerability
  Reporting pointer.
- SECURITY.md: drop the email line; rely on GitHub Private Vulnerability
  Reporting. (Operators enable PVR in repo settings — see 34.K.)

### 34.C — OSS hygiene files

Five new files. `CONTRIBUTING.md` mandates DCO sign-off but no CI job
enforces it — the new `dco.yml` workflow closes that gap.

- **`.github/PULL_REQUEST_TEMPLATE.md`** — linked-issue / Conventional
  Commit type / DCO sign-off / test-plan checklist.
- **`.github/CODEOWNERS`** — single-line `* @nyankobu010` for now;
  trivially extensible.
- **`.github/workflows/dco.yml`** — `tim-actions/dco@v1.1.0` on
  pull_request open / synchronize / reopened. Forward-only — pre-Phase-34
  commits are not retroactively signed.
- **`SUPPORT.md`** — three sections (Bugs / Questions / Vulnerabilities).
- **`CITATION.cff`** — academic-style citation with the four cited
  papers (TRINITY / Conductor / Fugu Beta / AB-MCTS).

### 34.D — Documentation refresh

The mkdocs site in `docs/` was last refreshed for v0.3.0 / Phase 2.5.
Phase 34 brings every page to v0.35.0 / Phase 34 parity.

- **`docs/index.md`** — full rewrite. Removed v0.3.0 release-notes
  paragraph; added a feature-area table covering the current shipping
  surface; added a per-phase roadmap table covering Phases 1–34.
- **`docs/architecture.md`** — full rewrite. Crate graph extended to
  include `tako-compat`, all seven SDK-backed provider crates, and
  per-provider URL pre-fetch helpers. Added a streaming-orchestrator
  sequence diagram. Removed the "describes Phase 1" closing paragraph;
  replaced with a current-state pointer to README.md / PLAN.md.
- **`docs/quickstart.md`** — replaced the "Phase 2 will add OTLP"
  forward-tense passage with the actual `init_otlp` builder call.
- **`docs/concepts/providers.md`** — added Mistral + Ollama rows;
  added Vision column; added vision-content code sample with an
  inline + URL-source example.
- **`docs/concepts/orchestrators.md`** — replaced the "Phase-4
  orchestrators (preview)" section with the shipping AbMcts surface,
  streaming events, and an OrchEvent table.
- **`docs/concepts/budgets.md`** — removed "Phase 4 will add Redis";
  documented `RedisBudgetBackend` as a shipping option, plus the
  `tako-runtime/redis` cargo feature gate.
- **`docs/concepts/mcp.md`** — removed "WebSocket and gRPC queued for
  Phase 4" / "Phase 3 will add tool sampling"; documented all four
  shipping transports (stdio / Streamable HTTP / WebSocket /
  gRPC mTLS) and the SSE notifications surface.
- **`docs/concepts/secrets.md`** — replaced "deferred" markers on
  Azure KV / GCP SM rows with "bring-your-own bearer" wording.

### 34.E — New documentation pages

Eight new pages cover features that shipped without docs.

#### Concepts

- **`concepts/vision.md`** — inline + URL-source content shapes per
  provider, supported MIME types, data-URL prefix normalisation,
  vendor-fetch vs. tako-pre-fetch path table.
- **`concepts/url_prefetch.md`** — opt-in URL fetcher SSRF mitigation
  stack: `https`-only / timeout / size cap / MIME validation /
  private-IP blocklist / DNS-rebind defence; per-host / wildcard /
  CIDR allowlist.
- **`concepts/streaming.md`** — `OrchEvent` variants, streaming
  guards (`RuleBasedGuard` + opt-in `LlmJudgeGuard` per-N-delta),
  streaming verifiers (`Verifier::evaluate_streaming`), branch
  identity, bounded `mpsc::channel(64)` backpressure.
- **`concepts/compat.md`** — OpenAI-compat HTTP server, `tako.*` SSE
  extensions, all five `AuthResolver` impls, `ChainedAuthResolver`
  with both fail-fast modes, every RFC 7662 / 8414 / 8705
  introspection auth method, end-session helper.
- **`concepts/sigstore.md`** — keyed + keyless verification modes,
  three `StateStore` backends (in-memory / JSON / Redis), cosign
  protobuf-bundle adapter, the full hard-fail enforcement list.

#### Recipes

- **`recipes/mistral.md`** — provider construction, streaming, vision,
  self-hosted gateway pointer.
- **`recipes/ollama.md`** — provider construction, streaming, inline
  vision via the sibling `images` field, URL-source via tako pre-fetch
  with allowlist examples.
- **`recipes/vision.md`** — inline + URL-source content end-to-end.
- **`recipes/url_prefetch.md`** — minimal config, allowlist forms
  (host / wildcard / CIDR), big-hammer override, verification snippet
  using `https://169.254.169.254/...`.
- **`recipes/oidc_introspection.md`** — discovery + JWKS, RFC 7662
  introspection, all six auth methods incl. `private_key_jwt` /
  mTLS, end-session helper.
- **`recipes/chained_auth.md`** — composite construction, fail-fast
  modes (transport-only / broader infrastructure), nesting.
- **`recipes/mtls_rotation.md`** — `reload_mtls_identity` from a
  filesystem-watcher pattern, atomicity guarantees, error-handling
  table.
- **`recipes/sigstore_keyless.md`** — `KeylessVerifier` with
  `TrustRoot` + `IdentityPolicy` + `JsonStateStore`, multi-replica
  `RedisStateStore`, cosign protobuf-bundle adapter usage.

### 34.F — Mkdocs nav update

`mkdocs.yml` nav extended with every new page from 34.E. Concepts
ordering preserves existing pages first, then groups the new pages
(streaming, vision, url_prefetch, compat, sigstore) by topic. Recipes
ordering integrates the new pages alongside the existing per-provider
recipes.

### 34.G — Sanity script

`scripts/check_public_release.sh` — bash, exit non-zero on any check
failure. Eight checks:

1. No `TODO(<org>)` outside historical files.
2. No `TODO(community)` outside historical files.
3. No tracked `.env` / `.pem` / `.key` / `id_rsa` / `credentials.json`.
4. Workspace + Python version strings agree.
5. `mkdocs build --strict` passes.
6. `cargo fmt --check`, `cargo clippy --all-features -D warnings`,
   `cargo test --workspace --all-features` pass.
7. `ruff check`, `ruff format --check`, `pytest -q tests/python` pass.

Each check is gated on the relevant tool being installed (graceful
"skipped" on missing tools so the script is useful in partial-CI
contexts).

### 34.H — CHANGELOG anchors + Phase 34 entry

- Compare-link footer extended from v0.14.0 (stale) to v0.34.0,
  switching the org from `TODO(<org>)` to `nyankobu010`.
- New `## [0.35.0] - <release-date>` section above the existing
  `## [0.34.0]`. The `## [Unreleased]` section is replaced (was empty
  per `(none)` placeholder).

### 34.I — Version bump 0.34.0 → 0.35.0

Workspace and Python both at 0.35.0.

- `Cargo.toml` workspace package + every `path = "..."` workspace dep.
- `pyproject.toml` project version.
- No source changes beyond the version constants — Phase 34 is
  hygiene + docs only.

### 34.J — PLAN.md index + roadmap update

- New row in the phase index: `34 — Public-release prep ... done (date)`.
- Replace the "Phase 34 candidates" section with "Phase 35 candidates"
  containing the trait-based `MtlsIdentityProvider`, automatic
  refresh-on-handshake-failure, filesystem-watcher integration,
  wildcard-at-non-leftmost-positions, strict-allowlist mode, OIDC mTLS
  end-to-end integration test, Vertex File API upload, eval graders,
  OIDC refresh-token / revocation, `TakoError::Provider` short-circuit,
  per-child `ChainedAuthResolver` policy override.
- Drop the resolved "First-publish placeholders" backlog rows.

### 34.K — Repo settings (out-of-tree, GitHub web UI)

Not in-repo work, but listed so it isn't forgotten:

- [ ] Repo at `github.com/nyankobu010/tako-ai-core` (rename or create).
- [ ] Enable Issues + Discussions.
- [ ] Enable Private Vulnerability Reporting (Settings → Code security
      and analysis).
- [ ] Default branch `main`.
- [ ] Repo description: `Rust-core, Python-facade framework for
      enterprise agentic systems. Many arms, one mind.`
- [ ] Topics: `rust`, `python`, `pyo3`, `agents`, `llm`, `mcp`,
      `apache-2`, `opentelemetry`, `mcts`.
- [ ] PyPI Trusted Publisher matching `wheels.yml` GitHub OIDC subject.

## Critical files

**Modified:** `Cargo.toml`, `pyproject.toml`, `mkdocs.yml`, `README.md`,
`CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CHANGELOG.md`,
`PLAN.md`, `crates/tako-core/src/lib.rs`, `docs/index.md`,
`docs/architecture.md`, `docs/quickstart.md`,
`docs/concepts/{providers,orchestrators,budgets,mcp,secrets,policy}.md`.

**Created:** `.github/PULL_REQUEST_TEMPLATE.md`, `.github/CODEOWNERS`,
`.github/workflows/dco.yml`, `SUPPORT.md`, `CITATION.cff`,
`scripts/check_public_release.sh`,
`docs/concepts/{vision,url_prefetch,streaming,compat,sigstore}.md`,
`docs/recipes/{mistral,ollama,vision,url_prefetch,oidc_introspection,chained_auth,mtls_rotation,sigstore_keyless}.md`,
`PLAN_PHASE34.md`.

**Read-only context:** `NOTICE`, `PLAN_PHASE1.md`, `PLAN_PHASE21.md`,
`PLAN_CLEANUP.md`.

## Verification

1. `git grep -nE 'TODO\(<org>\)|TODO\(community\)'` returns only the
   intentional historical hits.
2. `cargo fmt --all -- --check` passes.
3. `cargo clippy --workspace --all-features -- -D warnings` passes.
4. `cargo test --workspace --all-features` passes.
5. `ruff check python/ tests/python/ examples/` passes.
6. `pytest -q tests/python` passes.
7. `mkdocs build --strict` passes.
8. `bash scripts/check_public_release.sh` exits 0.
9. External clone smoke test:
   ```bash
   cd /tmp && rm -rf tako-ai-core
   git clone https://github.com/nyankobu010/tako-ai-core
   cd tako-ai-core && uv venv .venv && source .venv/bin/activate
   uv pip install -e '.[dev]' && maturin develop --release
   pytest -q
   ```

## Out of scope

- **No new features.** No code changes other than the rustdoc URL fix
  in `tako-core/src/lib.rs` and the version bump.
- **No NOTICE rewrite.** Already comprehensive; year stays 2026.
- **No license change.** Apache-2.0 stays.
- **No DCO history rewrite.** The new workflow enforces forward-only.
- **No new feature work** from the Phase 35 candidate list — those are
  the postponed items, not Phase 34 work.
- **No `PLAN_CLEANUP.md` deletion.** It's the historical document
  describing the placeholder strategy and is referenced from
  `PLAN_PHASE34.md` (this file). Leave in place; treat as a frozen
  historical artefact.
