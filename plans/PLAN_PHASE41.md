# PLAN — Phase 41 (Security: `jsonwebtoken` 9.3 → 10.3 bump)

> **Status: in progress.** Targets v0.42.0. Closes the
> dependabot advisory carry-forward from
> [Phase 39](PLAN_PHASE39.md#side-fix-revert-jsonwebtoken-9-10) —
> a security fix, not a feature phase.

## Context

Five open dependabot alerts on the repository at the start of
this phase:

| # | Severity | Crate | Range | Patched in |
|---|----------|-------|-------|------------|
| 5, 1 | medium | `jsonwebtoken` | < 10.3.0 | 10.3.0 |
| 4 | high | `rustls-webpki` | < 0.103.13 | 0.103.13 |
| 2, 3 | low | `rustls-webpki` | >= 0.101.0, < 0.103.12 | 0.103.12 |

Triage:

- **rustls-webpki** — three advisories, all transitive via
  `aws-smithy-http-client → rustls 0.21.12`. The current
  `Cargo.lock` already has `rustls-webpki 0.103.13` on the
  modern paths (high-severity DoS patched). The two
  low-severity issues only affect the legacy `0.101.x` line
  pinned by the AWS SDK chain; tako's URL-allowlist + URL
  pre-fetch surface doesn't parse CRLs or use URI-based name
  constraints. All three are documented + ignored in
  [`.cargo/audit.toml`](../.cargo/audit.toml) with mitigation
  rationales. They'll clear when AWS SDK migrates to rustls
  0.23+ (tracked in `awslabs/aws-sdk-rust#1295`). Phase 41
  dismisses the corresponding dependabot alerts pointing at
  the audit.toml entries — no code change needed.
- **jsonwebtoken** — direct dep. Real fix needed. The 9.x
  crate has a Type Confusion bug
  ([GHSA-vfgw-wj55-mp36](https://github.com/Keats/jsonwebtoken/security/advisories/GHSA-vfgw-wj55-mp36))
  patched in 10.3.0. PR #32 already attempted this bump, but
  the breaking-change handling was wrong: 10.x requires
  explicit selection of a `CryptoProvider` AND the PEM
  helpers moved behind a feature gate. Phase 39 reverted to
  9.3 to unblock; Phase 41 finishes the migration properly.

## Why now

- Two open alerts (medium severity, direct dep, exploitable
  in the introspection JWT validation path) need closing
  before any further tako-compat releases.
- The migration is mechanical now that the API surface is
  understood: 10.x retains the PEM helpers when `use_pem`
  feature is enabled. Just the dep config changes.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 41.A | jsonwebtoken 9.3 → 10.3 with `rust_crypto` + `use_pem` features | [`crates/tako-compat/Cargo.toml`](../crates/tako-compat/Cargo.toml), `Cargo.lock` |
| 41.B | Workspace + Python version 0.40.0 → 0.42.0 | various |
| 41.C | PLAN.md row + Phase 42 candidate refresh | [`PLAN.md`](../PLAN.md) |
| 41.D | CHANGELOG.md `[0.42.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |
| 41.E | Dismiss 3 rustls-webpki dependabot alerts | (out-of-tree, GitHub UI) |

## What this phase will land

### 41.A — jsonwebtoken 9.3 → 10.3

Single dep change in `crates/tako-compat/Cargo.toml`:

```diff
- jsonwebtoken = { version = "9.3", optional = true }
+ jsonwebtoken = { version = "10.3", default-features = false, features = ["rust_crypto", "use_pem"], optional = true }
```

- `default-features = false` drops `aws-lc-rs` (would add an
  OpenSSL / aws-lc-rs system-library dep we don't want).
- `rust_crypto` keeps the pure-Rust crypto stack (matches the
  rest of the workspace's `rustls`-based TLS choice).
- `use_pem` re-enables the PEM helper functions (`from_rsa_pem`
  / `from_ec_pem` / `from_ed_pem`) — `default = ["use_pem"]`
  upstream, but we set `default-features = false` to drop
  `aws-lc-rs`, so we have to enable it explicitly.

No source-code changes — every existing call site
(`jwt.rs:68,75`, `oidc.rs:213,225,238,2254`) keeps working
verbatim under the 10.x API once `use_pem` is on.

### 41.B — Version bump

Standard workspace + Python version flip 0.40.0 → 0.42.0.

### 41.C — PLAN.md update

- New row: `41 — Security: jsonwebtoken 9.3 -> 10.3 bump`.
- Phase 42 candidate list keeps the same items as Phase 41 candidates
  minus the (now-shipped) jsonwebtoken migration. The Python facade
  for `MtlsRefreshHook` (Phase 40, #38) is the only outstanding work
  in the auth-rotation surface; it'll rebase onto v0.42.0.

### 41.D — CHANGELOG entry

`## [0.42.0]` block with a Security section calling out
GHSA-vfgw-wj55-mp36 + the dropped `aws-lc-rs` system-dep
side-effect.

### 41.E — Dismiss rustls-webpki dependabot alerts (out-of-tree)

Three alerts (#2, #3, #4) dismissed via `gh` CLI with
`--reason tolerable_risk` and a comment pointing to the
[`.cargo/audit.toml`](../.cargo/audit.toml) entries:

- `RUSTSEC-2026-0104` (CRL panic)
- `RUSTSEC-2026-0098` (URI name constraints)
- `RUSTSEC-2026-0099` (wildcard cert name constraints)

All three are pinned by `rustls 0.21.12 ←
aws-smithy-http-client`. Tako's code paths don't exercise
CRLs / URI-name-constraint validation. Re-evaluate when AWS
bumps to rustls 0.23+ (tracked at
`awslabs/aws-sdk-rust#1295`).

## Critical files

**Modified:**
- [`crates/tako-compat/Cargo.toml`](../crates/tako-compat/Cargo.toml)
- `Cargo.lock`
- [`PLAN.md`](../PLAN.md), [`CHANGELOG.md`](../CHANGELOG.md)
- Version bump: `Cargo.toml`, `pyproject.toml`,
  `python/tako/__init__.py`, `tests/python/test_smoke.py`.

**Created:**
- [`plans/PLAN_PHASE41.md`](PLAN_PHASE41.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
3. `cargo test -p tako-compat --all-features` — all 153 lib
   tests + 8 server + 6 vault_token + 1 doctest pass under
   jsonwebtoken 10.3.
4. `cargo test --workspace --exclude tako-py --all-features`.
5. `cargo audit` returns no advisories (rustls-webpki ones
   stay ignored per audit.toml; jsonwebtoken now on 10.3).
6. `ruff format --check` + `ruff check`.
7. `pytest -q` — 254 passed.
8. `maturin develop --features "..."` — wheel builds at
   v0.42.0.

## Out of scope

- **Phase 40 (#38) rebase.** Phase 40 currently targets
  v0.42.0; after Phase 41 lands as v0.42.0, #38 will rebase
  to v0.42.0. Trivial mechanical conflict (PLAN.md /
  CHANGELOG.md / version files).
- **rustls 0.23 migration.** Out of our hands until AWS SDK
  bumps. Tracked in `awslabs/aws-sdk-rust#1295`.
- **`pem`-crate based wrapper layer.** Earlier exploration
  in this phase added a `pem_decode` helper module; deleted
  once it became clear the 10.x `use_pem` feature keeps the
  exact 9.x PEM helper API. The wrapper would have been pure
  overhead.
