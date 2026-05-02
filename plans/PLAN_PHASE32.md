# PLAN — Phase 32 (URL pre-fetch CIDR allowlist)

## Context

Phase 30 (v0.31.0) shipped exact-string per-host allowlist for
the URL pre-fetcher. Phase 31 (v0.32.0) extended that with
wildcard suffix patterns (`*.internal.corp`). Both forms operate
on the URL's host string.

Operators with a private subnet (a whole VPC, a Pod-CIDR,
etc.) hosting many hosts behind dynamic DNS or A/AAAA records
that change daily would have to enumerate every host as it
spawns:

```rust
.with_url_prefetch_allow_host("svc-a.internal.corp")
.with_url_prefetch_allow_host("svc-b.internal.corp")
.with_url_prefetch_allow_host("*.internal.corp")  // catches the suffix
// ... but what about hosts with no DNS at all? (raw IPs)
```

The Phase 31 wildcard helps with hostnames-under-a-suffix, but
two cases still hurt:
1. **Raw IP literals** — no hostname to wildcard. Each IP would
   be an exact-string entry.
2. **Subnets without a shared DNS suffix** — operator owns
   `10.0.5.0/24` but hosts there don't all share a DNS suffix
   (or have no DNS at all).

Phase 32 closes the gap with CIDR allowlists: operators chain
`with_url_prefetch_allow_cidr("10.0.5.0/24")` to permit any IP
in that subnet (whether reached via hostname resolution or as
an IP literal in the URL). After Phase 32 the operator allowlist
surface covers three semantically distinct forms:

| Form           | Example              | Match against         |
|----------------|----------------------|-----------------------|
| Exact string   | `"registry.corp"`    | URL host string       |
| Wildcard       | `"*.internal.corp"`  | URL host suffix       |
| CIDR subnet    | `"10.0.5.0/24"`      | Resolved IP (any)     |

## CIDR semantics

- Both IPv4 and IPv6 supported (`10.0.5.0/24`, `2001:db8::/32`).
- Single host as `/32` IPv4 or `/128` IPv6 is also valid (and
  equivalent to `with_url_prefetch_allow_host("10.0.5.4")` for
  the IP-literal path; for the hostname-resolves-to-this-IP
  path, the CIDR form is the only way to match).
- Bypass triggers when EITHER the URL's host string matches the
  Phase 30/31 allowlist OR the resolved IP (or IP literal) is
  in an allowlisted CIDR.
- The CIDR check is per-IP: in the resolver, each returned
  `SocketAddr` is independently checked against the CIDR list.
  An IP not in any CIDR (and not under an allowlisted hostname)
  must still pass `is_blocked_ip` to be allowed.

## Workspace dep

Phase 32 adds `ipnet = "2"` to workspace `[workspace.dependencies]`
([Cargo.toml](Cargo.toml)). `ipnet` is small (~7 KB), well-
maintained, no other transitive deps. Both Bedrock and Ollama
crates pick it up.

## A. Bedrock URL pre-fetch: CIDR allowlist

### A.1 — Extend `AllowList` with `cidrs: Vec<IpNet>`

[`crates/tako-providers/bedrock/src/url_prefetch.rs`](crates/tako-providers/bedrock/src/url_prefetch.rs):

```rust
pub(crate) struct AllowList {
    exact: HashSet<String>,
    suffixes: Vec<String>,
    /// Phase 32 — IPv4 + IPv6 CIDR networks. Bypass the
    /// blocklist for any resolved IP in any listed CIDR.
    cidrs: Vec<ipnet::IpNet>,
}

impl AllowList {
    pub(crate) fn from_strings_and_cidrs(
        host_entries: Vec<String>,
        cidr_entries: Vec<String>,
    ) -> Result<Self, TakoError> {
        // ... existing host parsing ...
        let mut cidrs = Vec::with_capacity(cidr_entries.len());
        for entry in cidr_entries {
            let cidr = entry.parse::<ipnet::IpNet>().map_err(|e| {
                TakoError::Invalid(format!(
                    "bedrock: prefetch CIDR `{entry}` parse failed: {e}"
                ))
            })?;
            cidrs.push(cidr);
        }
        Ok(Self { exact, suffixes, cidrs })
    }

    pub(crate) fn contains(&self, host: &str) -> bool { /* unchanged */ }

    pub(crate) fn contains_ip(&self, ip: &std::net::IpAddr) -> bool {
        self.cidrs.iter().any(|net| net.contains(ip))
    }
}
```

`from_strings` (Phase 31 signature) is renamed to
`from_strings_and_cidrs(hosts, cidrs)`. Callers updated. CIDR
parse failures surface as `TakoError::Invalid` at builder time
so operators notice early — consistent with Phase 24/25 mTLS
PEM parse-time failure cadence.

### A.2 — `BlocklistResolver` + `fetch_one` honour CIDRs

```rust
// In BlocklistResolver::resolve:
for addr in &addrs {
    if !allow_hosts.contains(&host)
        && !allow_hosts.contains_ip(&addr.ip())
        && is_blocked_ip(&addr.ip())
    {
        return Err(...);
    }
}

// In fetch_one IP-literal check:
if !self.allow_hosts.contains(host_str)
    && !self.allow_hosts.contains_ip(&ip)
    && is_blocked_ip(&ip)
{
    return Err(...);
}
```

The combined check is "skip blocklist if the host is
allowlisted OR the IP is in an allowlisted CIDR". Phase 30/31
behaviour preserved when the CIDR list is empty.

### A.3 — `UrlPrefetchOpts.allow_cidrs: Vec<String>` builder field

```rust
pub(crate) struct UrlPrefetchOpts {
    // ... existing fields ...
    /// Phase 32 — CIDR strings to bypass the private-IP
    /// blocklist when a resolved IP falls inside the network.
    /// Default empty.
    pub(crate) allow_cidrs: Vec<String>,
}
```

`Default::default()` initialises empty. `into_config` passes
`(self.allow_hosts, self.allow_cidrs)` into
`AllowList::from_strings_and_cidrs`.

### A.4 — `BedrockBuilder::with_url_prefetch_allow_cidr(cidr)`

[`crates/tako-providers/bedrock/src/client.rs`](crates/tako-providers/bedrock/src/client.rs):

```rust
pub fn with_url_prefetch_allow_cidr(mut self, cidr: impl Into<String>) -> Self {
    self.url_prefetch.allow_cidrs.push(cidr.into());
    self
}
```

Chainable. Does NOT auto-enable `with_url_prefetch()`. CIDR
parse failures surface from `build()` (which calls
`into_config`) as `TakoError::Invalid`.

### A.5 — Tests

Unit tests in `url_prefetch.rs`:
- `allow_list_cidr_v4_match` — `10.0.5.0/24` contains `10.0.5.42`.
- `allow_list_cidr_v4_no_match` — `10.0.5.0/24` does NOT contain `10.0.6.42`.
- `allow_list_cidr_v6_match` — `2001:db8::/32` contains `2001:db8::1`.
- `allow_list_cidr_v6_no_match` — `2001:db8::/32` does NOT contain `2001:db9::1`.
- `allow_list_cidr_single_host_32` — `192.168.1.5/32` contains exactly that one IP.
- `allow_list_cidr_invalid_parse_returns_err` — `"not-a-cidr"` returns `TakoError::Invalid`.
- `allow_list_cidr_and_host_coexist` — exact host + wildcard host
  + CIDR all coexist in one allowlist.
- `rewrite_allowlists_ip_literal_via_cidr` — wiremock on `127.0.0.1`
  with `cidrs=["127.0.0.0/8"]`; rewrite succeeds.

Phase 30 + 31 tests pass byte-for-byte unchanged (CIDR list
defaults to empty).

## B. Ollama URL pre-fetch: same CIDR support

Per ARCHITECTURE.md hard rule, mirror in:
- [`crates/tako-providers/ollama/src/url_prefetch.rs`](crates/tako-providers/ollama/src/url_prefetch.rs)
- [`crates/tako-providers/ollama/src/client.rs`](crates/tako-providers/ollama/src/client.rs)

Same surface as A. Same test surface.

## C. Python facade: `url_prefetch_allow_cidrs` kwarg

### C.1 — Extend `PyBedrock::new()` and `PyOllama::new()`

Both add a new kwarg between `url_prefetch_allow_hosts` and
`url_prefetch_timeout_secs`:

```python
url_prefetch_allow_cidrs: list[str] | None = None
```

When `Some(cidrs)`, the PyO3 ctor calls
`with_url_prefetch_allow_cidr(cidr)` for each entry on the
underlying builder. `None` (default) means empty CIDR list.

### C.2 — Python-side mirrors

[`python/tako/providers.py`](python/tako/providers.py): add
`url_prefetch_allow_cidrs: list[str] | None = None` to both
`Bedrock` and `Ollama` `__init__`. Both docstrings updated to
document the new kwarg with usage examples.

[`python/tako/_native.pyi`](python/tako/_native.pyi): same on
both stubs.

### C.3 — Tests

[`tests/python/test_phase32_allow_cidrs.py`](tests/python/test_phase32_allow_cidrs.py)
(NEW) — signature smoke + docstring pin, mirroring Phase 30.C
style.

## Out of scope (deferred to Phase 33+)

- **Wildcard at non-leftmost positions** — patterns like
  `registry.*.corp`. Probably never worth shipping unless a
  real operator asks.
- **Strict-allowlist mode** — currently all allowlists are
  per-rule BYPASSes of the blocklist. A strict mode would
  REQUIRE every URL host to match an allowlist entry (no
  bypass; reject everything else). Out of scope for Phase 32.
- **OIDC mTLS end-to-end integration test** (Phase 24/25
  carry-forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).
- **`TakoError::Provider` short-circuit on `ChainedAuthResolver`**
  (Phase 27 carry-forward).

## Acceptance criteria

- `cargo test -p tako-providers-bedrock --all-features` passes
- `cargo test -p tako-providers-ollama --all-features` passes
- `cargo test -p tako-py --all-features` passes
- `cargo clippy --workspace --all-features -- -D warnings` passes
- `cargo fmt --all -- --check` passes
- `pytest tests/python/test_phase32_allow_cidrs.py` passes
- `pytest tests/python/test_phase{28,29,30,31}_*.py` pass (regressions)
- `pytest -q` passes (after `maturin develop --release`)

## Commit cadence

1. `docs: PLAN_PHASE32.md`
2. `feat(tako-providers/bedrock): URL pre-fetch CIDR allowlist (Phase 32.A)`
3. `feat(tako-providers/ollama): URL pre-fetch CIDR allowlist (Phase 32.B)`
4. `feat(tako-py): url_prefetch_allow_cidrs kwarg on Bedrock + Ollama (Phase 32.C)`
5. `docs: Phase 32 PLAN/README/CHANGELOG flip (v0.33.0)`
