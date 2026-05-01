# PLAN — Phase 30 (URL pre-fetch per-host allowlist)

## Context

Phase 29 (v0.30.0) shipped a default-on private-IP blocklist for
the tako-side URL pre-fetcher: loopback / RFC 1918 / link-local /
multicast / IPv6 unique-local + link-local + IPv4-mapped variants
are all rejected at DNS-resolve time, with an inline IP-literal
check covering URLs whose host is already an IP. Operators with
deployment-level egress filtering can flip the whole blocklist
off via `with_url_prefetch_allow_private_ips()`.

The binary on/off knob doesn't fit a real operator pattern: a
deployment with an internal artifact registry on
`internal-registry.corp.local` (resolving to `10.0.5.4` say)
wants to permit URL pre-fetch from that one host while still
blocking everything else on `10/8`, `127/8`, `169.254/16`, etc.
Phase 29's `with_url_prefetch_allow_private_ips()` is a
sledgehammer — it allows ALL private addresses, including the
canary `169.254.169.254` cloud-metadata endpoint.

Phase 30 closes that gap with a per-host allowlist. Operators
chain `with_url_prefetch_allow_host("internal-registry.corp.local")`
on the builder; URLs whose host matches an entry in the allowlist
bypass the private-IP blocklist for that host only. Other hosts
still hit the full blocklist. Defence-in-depth: scheme check,
timeout, size cap, MIME validation all still apply unchanged.

The allowlist is pure exact-string match (no wildcards / suffix
patterns). Wildcard patterns (`*.internal.corp.local`) and CIDR
allowlists land in Phase 31+ if there's demand.

## A. Bedrock URL pre-fetch: per-host allowlist

### A.1 — `UrlPrefetchConfig.allow_hosts` field

[`crates/tako-providers/bedrock/src/url_prefetch.rs`](crates/tako-providers/bedrock/src/url_prefetch.rs):

```rust
pub(crate) struct UrlPrefetchConfig {
    pub(crate) allow_http: bool,
    pub(crate) max_bytes: usize,
    pub(crate) block_private_ips: bool,
    pub(crate) http: reqwest::Client,
    /// Phase 30 — per-host allowlist. Hostnames in this set
    /// bypass the private-IP blocklist (but NOT the scheme /
    /// timeout / size / MIME checks). Empty by default.
    pub(crate) allow_hosts: Arc<HashSet<String>>,
}
```

The allowlist is shared across the resolver and `fetch_one` via
`Arc<HashSet<String>>`. Cloning is cheap (Arc bump only).

### A.2 — `BlocklistResolver` carries the allowlist

```rust
struct BlocklistResolver {
    allow_hosts: Arc<HashSet<String>>,
}

impl Resolve for BlocklistResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let allow_hosts = self.allow_hosts.clone();
        Box::pin(async move {
            let addrs = tokio::net::lookup_host(...)?.collect();
            // Phase 30 — bypass the blocklist for allowlisted hosts.
            if !allow_hosts.contains(&host) {
                for addr in &addrs {
                    if is_blocked_ip(&addr.ip()) {
                        return Err(...);
                    }
                }
            }
            Ok(...)
        })
    }
}
```

The resolver is consulted only when `block_private_ips` is on
AND the URL has a hostname (not an IP literal). The allowlist
inside the resolver is the per-host bypass.

### A.3 — `fetch_one` IP-literal check honours the allowlist

```rust
if self.block_private_ips {
    if let Some(host_str) = parsed.host_str() {
        let trimmed = host_str.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = trimmed.parse::<IpAddr>() {
            // Phase 30 — even IP literals can be allowlisted
            // by their literal host_str (e.g.
            // `with_url_prefetch_allow_host("10.0.5.4")`).
            if !self.allow_hosts.contains(host_str) && is_blocked_ip(&ip) {
                return Err(...);
            }
        }
    }
}
```

Note: the allowlist is matched against the raw `host_str` not
the parsed `IpAddr`, so `with_url_prefetch_allow_host("10.0.5.4")`
matches a URL `http://10.0.5.4/...` but NOT `http://10.0.5.4./...`
(trailing dot — a different DNS spelling).

### A.4 — `UrlPrefetchOpts.allow_hosts` builder field

```rust
#[derive(Debug, Clone)]
pub(crate) struct UrlPrefetchOpts {
    pub(crate) enabled: bool,
    pub(crate) allow_http: bool,
    pub(crate) timeout: Option<Duration>,
    pub(crate) max_bytes: Option<usize>,
    pub(crate) block_private_ips: bool,
    /// Phase 30 — host strings to bypass the private-IP
    /// blocklist. Default empty.
    pub(crate) allow_hosts: Vec<String>,
}
```

`Default` impl initialises `allow_hosts: Vec::new()`. `into_config`
converts to `Arc<HashSet<String>>` once at build time.

### A.5 — `BedrockBuilder::with_url_prefetch_allow_host(host)`

[`crates/tako-providers/bedrock/src/client.rs`](crates/tako-providers/bedrock/src/client.rs):

```rust
pub fn with_url_prefetch_allow_host(mut self, host: impl Into<String>) -> Self {
    self.url_prefetch.allow_hosts.push(host.into());
    self
}
```

Chainable; can be called multiple times to add more hosts. Does
NOT auto-enable `with_url_prefetch()` (master switch must already
be on).

### A.6 — Tests

Unit tests in `url_prefetch.rs`:
- `allow_hosts_empty_by_default` — `UrlPrefetchOpts::default().allow_hosts.is_empty()`
- `allow_hosts_into_config_round_trip` — `Vec<String>` → `Arc<HashSet<String>>`
  preserves all entries
- `allow_hosts_dedupes` — adding the same host twice is a noop
  on the resulting HashSet (count == 1)
- `rewrite_allowlists_ip_literal_host` — wiremock on
  `127.0.0.1`; with `block_private_ips: true` and
  `allow_hosts = {"127.0.0.1"}`, the rewrite succeeds.
- `rewrite_blocks_ip_literal_not_in_allowlist` — same setup
  but with `allow_hosts = {"different-host"}`; rewrite fails
  with the blocklist error.

Phase 28 + 29 tests pass byte-for-byte unchanged (the new field
defaults to empty so existing semantics preserved).

## B. Ollama URL pre-fetch: same per-host allowlist

Per-crate copy of A in:
- [`crates/tako-providers/ollama/src/url_prefetch.rs`](crates/tako-providers/ollama/src/url_prefetch.rs)
- [`crates/tako-providers/ollama/src/client.rs`](crates/tako-providers/ollama/src/client.rs)

Same surface as A. Per ARCHITECTURE.md hard rule (provider
crates depend only on `tako-core` + their vendor SDK + reqwest;
never on each other).

## C. Python facade: `url_prefetch_allow_hosts` kwarg on Bedrock + Ollama

### C.1 — Extend `PyBedrock::new()`

[`crates/tako-py/src/py_bedrock.rs`](crates/tako-py/src/py_bedrock.rs):

```rust
#[pyo3(signature = (
    model,
    region=None,
    endpoint_url=None,
    profile_name=None,
    url_prefetch=false,
    url_prefetch_allow_http=false,
    url_prefetch_allow_private_ips=false,
    url_prefetch_allow_hosts=None,
    url_prefetch_timeout_secs=None,
    url_prefetch_max_bytes=None,
))]
fn new(
    py: Python<'_>,
    model: String,
    region: Option<String>,
    endpoint_url: Option<String>,
    profile_name: Option<String>,
    url_prefetch: bool,
    url_prefetch_allow_http: bool,
    url_prefetch_allow_private_ips: bool,
    url_prefetch_allow_hosts: Option<Vec<String>>,
    url_prefetch_timeout_secs: Option<u64>,
    url_prefetch_max_bytes: Option<usize>,
) -> PyResult<Self> {
    ...
    if let Some(hosts) = url_prefetch_allow_hosts {
        for host in hosts {
            b = b.with_url_prefetch_allow_host(host);
        }
    }
    ...
}
```

`url_prefetch_allow_hosts: list[str] | None = None` — `None`
means empty allowlist (default behaviour). Python list
serialises to `Vec<String>` via PyO3.

### C.2 — Extend `PyOllama::new()`

[`crates/tako-py/src/py_ollama.rs`](crates/tako-py/src/py_ollama.rs):
mirror of C.1.

### C.3 — Python-side mirrors

[`python/tako/providers.py`](python/tako/providers.py): add
`url_prefetch_allow_hosts: list[str] | None = None` to both
`Bedrock` and `Ollama` `__init__`. Update both docstrings to
document the new kwarg.

[`python/tako/_native.pyi`](python/tako/_native.pyi): same on
both stubs.

### C.4 — Tests

[`tests/python/test_phase30_allow_hosts.py`](tests/python/test_phase30_allow_hosts.py)
(NEW) — signature smoke:
- New `url_prefetch_allow_hosts` kwarg present + default `None`
  on both `Bedrock` and `Ollama`.
- Docstring documents the new kwarg.

Style mirrors Phase 29's
`tests/python/test_phase29_ssrf_hardening.py`.

## Out of scope (deferred to Phase 31+)

- **Wildcard / suffix patterns in the allowlist** — operators
  may want `*.internal.corp.local` to allow all subdomains.
  Phase 30 ships exact-string match only.
- **CIDR allowlist** — operators may want
  `with_url_prefetch_allow_cidr("10.0.5.0/24")`. Phase 30
  ships hostname-only allowlist; CIDR matching needs a CIDR
  parser dep.
- **Strict-allowlist mode** — currently the allowlist is a
  per-host BYPASS of the blocklist. A strict-allowlist mode
  would only permit allowlisted hosts (everything else
  blocked).
- **OIDC mTLS end-to-end integration test** (Phase 24/25
  carry-forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).

## Acceptance criteria

- `cargo test -p tako-providers-bedrock --all-features` passes
- `cargo test -p tako-providers-ollama --all-features` passes
- `cargo test -p tako-py --all-features` passes
- `cargo clippy --workspace --all-features -- -D warnings` passes
- `cargo fmt --all -- --check` passes
- `pytest tests/python/test_phase30_allow_hosts.py` passes
- `pytest tests/python/test_phase{28,29}_*.py` pass (regressions)
- `pytest -q` passes (after `maturin develop --release`)

## Commit cadence

1. `docs: PLAN_PHASE30.md`
2. `feat(tako-providers/bedrock): URL pre-fetch per-host allowlist (Phase 30.A)`
3. `feat(tako-providers/ollama): URL pre-fetch per-host allowlist (Phase 30.B)`
4. `feat(tako-py): url_prefetch_allow_hosts kwarg on Bedrock + Ollama (Phase 30.C)`
5. `docs: Phase 30 PLAN/README/CHANGELOG flip (v0.31.0)`
