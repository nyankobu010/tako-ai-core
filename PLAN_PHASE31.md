# PLAN — Phase 31 (URL pre-fetch wildcard host patterns)

## Context

Phase 30 (v0.31.0) shipped a per-host allowlist for the
tako-side URL pre-fetcher: operators chain
`with_url_prefetch_allow_host("internal-registry.corp.local")`
to permit specific hostnames behind private RFC 1918 addresses
without disabling the whole Phase 29 blocklist.

Phase 30 ships exact-string match only. Operators with multiple
internal subdomains under one domain — common for deployments
with `registry.internal.corp`, `images.internal.corp`,
`packages.internal.corp` etc — would have to enumerate each
hostname:

```rust
.with_url_prefetch_allow_host("registry.internal.corp")
.with_url_prefetch_allow_host("images.internal.corp")
.with_url_prefetch_allow_host("packages.internal.corp")
// ... and remember to add new ones as they spawn
```

That's mostly busywork. Phase 31 adds wildcard suffix
patterns: a single `*.internal.corp` entry covers all current
and future subdomains under that suffix. The Phase 30 builder
surface (`with_url_prefetch_allow_host(host)`) and the
Python kwarg (`url_prefetch_allow_hosts: list[str] | None`) are
unchanged — entries starting with `*.` are recognised as
wildcards at config time, everything else stays exact.

## Wildcard semantics

`*.X` matches any hostname `Y` such that `Y.ends_with(".X")`
literally — including multi-level subdomains:

| Pattern             | Hostname                          | Match? |
|---------------------|-----------------------------------|--------|
| `*.internal.corp`   | `registry.internal.corp`          | ✅      |
| `*.internal.corp`   | `staging.images.internal.corp`    | ✅      |
| `*.internal.corp`   | `internal.corp`                   | ❌ (no preceding dot) |
| `*.internal.corp`   | `evil.com`                        | ❌      |
| `*.internal.corp`   | `attacker-internal.corp`          | ❌ (no preceding dot before `internal`) |

Operator intent is "any subdomain of internal.corp" — this is
the standard `*.example.com` glob convention used by TLS cert
SANs (RFC 6125), but with multi-level matching enabled (RFC
6125 strictly says one level only; for an opt-in operator-
controlled allowlist, multi-level is the more useful default).

## A. Bedrock URL pre-fetch: wildcard patterns

### A.1 — `AllowList` struct

[`crates/tako-providers/bedrock/src/url_prefetch.rs`](crates/tako-providers/bedrock/src/url_prefetch.rs):

```rust
/// Phase 31 — split exact-match hostnames from wildcard suffix
/// patterns at config time so `BlocklistResolver` and the
/// inline IP-literal check don't re-parse on every request.
#[derive(Debug, Default)]
pub(crate) struct AllowList {
    exact: HashSet<String>,
    /// Each entry stored as `.X` (ready for `ends_with`).
    /// Phase 30 entries (no `*.` prefix) go in `exact`; Phase
    /// 31 entries starting with `*.` get the leading `*` stripped
    /// and the result stored here.
    suffixes: Vec<String>,
}

impl AllowList {
    pub(crate) fn from_strings(entries: Vec<String>) -> Self {
        let mut exact = HashSet::new();
        let mut suffixes = Vec::new();
        for entry in entries {
            if let Some(suffix) = entry.strip_prefix("*.") {
                // Store as `.X` so `host.ends_with(s)` does the
                // right thing without per-call format!().
                suffixes.push(format!(".{suffix}"));
            } else {
                exact.insert(entry);
            }
        }
        Self { exact, suffixes }
    }

    pub(crate) fn contains(&self, host: &str) -> bool {
        self.exact.contains(host)
            || self.suffixes.iter().any(|s| host.ends_with(s.as_str()))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.suffixes.is_empty()
    }
}
```

### A.2 — Replace `Arc<HashSet<String>>` with `Arc<AllowList>`

`UrlPrefetchConfig.allow_hosts: Arc<HashSet<String>>` →
`Arc<AllowList>`. `BlocklistResolver` and the inline IP-literal
check call `allow_hosts.contains(host)` exactly the same way —
the `AllowList::contains` method handles both exact and suffix
matching. Phase 30 callers (`HashSet::contains`) just need the
field type swap.

`UrlPrefetchOpts.into_config` calls `AllowList::from_strings`
to convert the builder-side `Vec<String>` into the runtime
`Arc<AllowList>`.

### A.3 — Update `BedrockBuilder::with_url_prefetch_allow_host` doc

The builder method itself doesn't change — entries starting
with `*.` are recognised at `into_config` time. Update the doc
comment to mention wildcard support and link to the Phase 31
semantic.

### A.4 — Tests

Unit tests in `url_prefetch.rs`:
- `allow_list_default_is_empty`
- `allow_list_exact_match` — Phase 30 regression.
- `allow_list_wildcard_matches_subdomain` — `*.internal.corp` matches
  `registry.internal.corp`.
- `allow_list_wildcard_matches_multi_level` — `*.internal.corp`
  matches `staging.images.internal.corp`.
- `allow_list_wildcard_does_not_match_bare_domain` —
  `*.internal.corp` does NOT match `internal.corp`.
- `allow_list_wildcard_does_not_match_other_domain` —
  `*.internal.corp` does NOT match `evil.com`.
- `allow_list_wildcard_does_not_match_attacker_domain` —
  `*.internal.corp` does NOT match `attacker-internal.corp`.
- `allow_list_exact_and_wildcard_coexist` — allowlist with
  both `registry.public.com` and `*.internal.corp` matches
  both kinds.
- `rewrite_allowlists_wildcard_subdomain` — wiremock on
  `127.0.0.1` (we'll allowlist `*.0.0.1` since wiremock binds
  to a literal IP — actually this won't work well; let me use
  `*` style at IP level or a hostname in /etc/hosts...).

  Actually this one is awkward because wiremock binds to
  `127.0.0.1` which is an IP literal, not a hostname. The
  resolver only sees hostnames. The IP-literal check uses raw
  host_str so wildcard semantics don't apply to IP literals
  (a `*.0.0.1` pattern wouldn't be a valid wildcard anyway —
  IPs don't have suffixes).
  
  Cleaner: pure unit test on the `AllowList::contains` method
  for wildcard semantics; integration test reuses the existing
  Phase 30 IP-literal allowlist test (validates that the new
  data structure preserves Phase 30 behaviour).

Replace the wiremock integration test sketch above with: keep
the Phase 30 IP-literal test as a regression pin (Phase 31's
internal struct refactor must preserve byte-for-byte semantics
for exact-string matches).

## B. Ollama URL pre-fetch: same wildcard support

Per ARCHITECTURE.md hard rule (provider crates depend only on
`tako-core` + their vendor SDK + reqwest; never on each other),
the `AllowList` struct + matching logic is duplicated in:
- [`crates/tako-providers/ollama/src/url_prefetch.rs`](crates/tako-providers/ollama/src/url_prefetch.rs)

Same surface as A. Same tests. No `OllamaBuilder` changes —
the existing `with_url_prefetch_allow_host(host)` builder
accepts wildcard entries unchanged.

## C. Python facade: docstring updates + Python-side tests

### C.1 — No code change in `crates/tako-py/src/`

The Python kwarg `url_prefetch_allow_hosts: list[str] | None`
already accepts arbitrary strings; the new wildcard semantic
lands entirely on the Rust side. No PyO3 change needed.

### C.2 — Update Python docstrings

[`python/tako/providers.py`](python/tako/providers.py) on both
`Bedrock` and `Ollama` — extend the
`url_prefetch_allow_hosts` paragraph to mention wildcard
patterns (`"*.internal.corp"`) with the multi-level matching
semantic.

### C.3 — New tests

[`tests/python/test_phase31_wildcard_hosts.py`](tests/python/test_phase31_wildcard_hosts.py)
(NEW) — signature smoke + docstring pin:
- `url_prefetch_allow_hosts` kwarg still accepts `list[str] | None`
  (regression).
- Both providers' docstrings mention `*.` wildcard patterns
  + multi-level matching semantic.

Style mirrors Phase 30's `tests/python/test_phase30_allow_hosts.py`.

## Out of scope (deferred to Phase 32+)

- **Wildcard at non-leftmost positions** — patterns like
  `registry.*.corp` (wildcard in middle). Phase 31 ships only
  the leftmost-`*.` convention.
- **Multiple wildcards in one pattern** — `*.*.corp`. Out of
  scope; would need real glob semantics.
- **CIDR allowlist** — `with_url_prefetch_allow_cidr("10.0.5.0/24")`.
  Carry-forward; needs a CIDR parser dep.
- **OIDC mTLS end-to-end integration test** (Phase 24/25
  carry-forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).

## Acceptance criteria

- `cargo test -p tako-providers-bedrock --all-features` passes
- `cargo test -p tako-providers-ollama --all-features` passes
- `cargo test -p tako-py --all-features` passes
- `cargo clippy --workspace --all-features -- -D warnings` passes
- `cargo fmt --all -- --check` passes
- `pytest tests/python/test_phase31_wildcard_hosts.py` passes
- `pytest tests/python/test_phase{28,29,30}_*.py` pass (regressions)
- `pytest -q` passes (after `maturin develop --release`)

## Commit cadence

1. `docs: PLAN_PHASE31.md`
2. `feat(tako-providers/bedrock): URL pre-fetch wildcard host patterns (Phase 31.A)`
3. `feat(tako-providers/ollama): URL pre-fetch wildcard host patterns (Phase 31.B)`
4. `docs(tako-py): document wildcard host patterns on Bedrock + Ollama (Phase 31.C)`
5. `docs: Phase 31 PLAN/README/CHANGELOG flip (v0.32.0)`
