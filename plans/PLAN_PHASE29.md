# PLAN — Phase 29 (URL pre-fetch SSRF hardening + Ollama Python facade)

## Context

Phase 28 (v0.29.0) shipped opt-in tako-side URL pre-fetch on Bedrock + Ollama
to close the URL-source-image gap on the two providers whose vendors don't
fetch URLs themselves. Phase 28's SSRF mitigations were:

1. Opt-in (default silent-drop).
2. `https://`-only by default.
3. Configurable timeout (default 10s).
4. Configurable size cap (default 10 MiB; `Content-Length` pre-flight +
   post-fetch byte-count defence-in-depth).
5. MIME validation against `image/{jpeg,png,gif,webp}`.

Phase 28's plan explicitly deferred two more mitigations to Phase 29+:

> Out of scope for Phase 28: CIDR blocklist for private / link-local IPs,
> DNS-rebinding mitigation. Operators must enforce network egress policy
> at deployment level (via VPC egress rules, Pod-level egress
> NetworkPolicies, etc.). Phase 29+ may add per-request CIDR check +
> resolve-once-then-connect.

Without those mitigations, an attacker who can inject `ContentPart::ImageUrl`
into a request can ask tako to fetch:
- `https://169.254.169.254/...` — cloud-instance metadata endpoint
  (AWS / GCP / Azure all expose secrets via this RFC 3927 link-local IP)
- `https://10.0.0.1/admin` — internal admin services on private RFC 1918
  ranges
- `https://localhost/...` — services bound to loopback on the same
  pod / VM

Phase 29 closes that gap with **defence-in-depth**: a default-on
private/loopback/link-local IP blocklist enforced at DNS-resolve time via a
custom `reqwest::dns::Resolve` impl. Because the resolver is the *only*
place the hostname → IP mapping happens, validating ALL returned IPs at
resolve time also forms a complete DNS-rebinding mitigation — a malicious
authoritative resolver can't slip a private IP through alongside a public
one, and there's no second resolution to exploit.

Phase 28.C also left a small Python-facade asymmetry:
`tako.providers.Bedrock` gained the four `url_prefetch_*` kwargs, but
`tako.providers.Ollama` doesn't exist yet (no `py_ollama.rs`, no
`Ollama` stub in `_native.pyi`). Phase 29.C closes that by mirroring the
Phase 28.C `PyBedrock` cadence.

After Phase 29:
- The tako-side URL pre-fetch surface ships with a complete SSRF-mitigation
  stack (Phase 28's https-only / timeout / size cap / MIME **plus** Phase
  29's private-IP blocklist + DNS-rebinding mitigation).
- Both URL-prefetching providers (Bedrock + Ollama) have full Python
  parity.

## A. Bedrock URL pre-fetch: private-IP blocklist + DNS-rebind mitigation

### A.1 — `is_blocked_ip()` helper

[`crates/tako-providers/bedrock/src/url_prefetch.rs`](crates/tako-providers/bedrock/src/url_prefetch.rs):

```rust
fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] >= 224  // multicast (224/4) + reserved (240/4)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // unique-local (fc00::/7)
                || (v6.segments()[0] & 0xfe00 == 0xfc00)
                // unicast-link-local (fe80::/10)
                || (v6.segments()[0] & 0xffc0 == 0xfe80)
                // IPv4-mapped: recurse on the embedded IPv4
                || v6.to_ipv4_mapped().is_some_and(|v4| is_blocked_ip(&IpAddr::V4(v4)))
        }
    }
}
```

Pure stdlib; no new deps. All called methods stable on the workspace
MSRV (1.85; `to_ipv4_mapped()` stabilised in 1.85).

### A.2 — `BlocklistResolver` impl of `reqwest::dns::Resolve`

`reqwest::dns::Resolve` is a public trait with a single
`resolve(&self, name: Name) -> Resolving` method (where `Resolving` is
`Pin<Box<dyn Future<Output = Result<Addrs, BoxError>> + Send>>`). The
custom resolver:

1. Calls `tokio::net::lookup_host((name.as_str(), 0))` to resolve the
   hostname to a `Vec<SocketAddr>`.
2. For EVERY returned `SocketAddr`, runs `is_blocked_ip(&addr.ip())`.
3. If any address is blocked, returns
   `io::Error::new(io::ErrorKind::PermissionDenied, "prefetch URL resolves to blocked IP …")`.
4. Otherwise wraps the addresses in a `Box<dyn Iterator<Item =
   SocketAddr> + Send>` (the `Addrs` type alias) and returns `Ok(addrs)`.

The blocklist runs at resolve-time, before the connection attempt. This
also closes the DNS-rebinding window: there's no second DNS lookup
between validation and connection.

### A.3 — `UrlPrefetchConfig.block_private_ips: bool` field

Add a new field to `UrlPrefetchConfig`:

```rust
pub(crate) struct UrlPrefetchConfig {
    pub(crate) allow_http: bool,
    pub(crate) max_bytes: usize,
    pub(crate) http: reqwest::Client,
}
```

becomes (the only externally observable difference is the resolver
inside `http`):

```rust
pub(crate) struct UrlPrefetchConfig {
    pub(crate) allow_http: bool,
    pub(crate) max_bytes: usize,
    pub(crate) http: reqwest::Client,  // resolver installed if block_private_ips
}
```

`UrlPrefetchConfig::new()` widens to take a `block_private_ips: bool`
parameter and conditionally installs the resolver:

```rust
pub(crate) fn new(
    allow_http: bool,
    timeout: Duration,
    max_bytes: usize,
    block_private_ips: bool,
) -> Result<Self, TakoError> {
    let mut builder = reqwest::Client::builder().timeout(timeout);
    if block_private_ips {
        builder = builder.dns_resolver(Arc::new(BlocklistResolver));
    }
    let http = builder.build().map_err(|e| ...)?;
    Ok(Self { allow_http, max_bytes, http })
}
```

`UrlPrefetchOpts.block_private_ips: bool` (default `true`); plumbed
through `into_config()`.

### A.4 — `BedrockBuilder::with_url_prefetch_allow_private_ips()`

[`crates/tako-providers/bedrock/src/client.rs`](crates/tako-providers/bedrock/src/client.rs):

```rust
pub fn with_url_prefetch_allow_private_ips(mut self) -> Self {
    self.url_prefetch.block_private_ips = false;
    self
}
```

Default-deny semantics: the method is the opt-out, mirroring Phase 28's
`with_url_prefetch_allow_http`. Idempotent. Setting it does NOT
auto-enable `url_prefetch.enabled` (operator must already have called
`with_url_prefetch()` for the flag to do anything).

### A.5 — Tests

Unit tests in `url_prefetch.rs`:
- `is_blocked_ip_blocks_loopback_v4` — `127.0.0.1`, `127.255.255.254`
- `is_blocked_ip_blocks_private_v4` — `10.0.0.1`, `172.16.0.1`,
  `172.31.255.255`, `192.168.0.1`
- `is_blocked_ip_blocks_link_local_v4` — `169.254.0.1`,
  `169.254.169.254` (the cloud-metadata canary)
- `is_blocked_ip_blocks_loopback_v6` — `::1`
- `is_blocked_ip_blocks_unique_local_v6` — `fc00::1`, `fd00::ffff`
- `is_blocked_ip_blocks_link_local_v6` — `fe80::1`
- `is_blocked_ip_blocks_v4_mapped_loopback` — `::ffff:127.0.0.1`
- `is_blocked_ip_allows_public_v4` — `8.8.8.8`, `1.1.1.1`
- `is_blocked_ip_allows_public_v6` — `2001:db8::1`
- `opts_into_config_default_blocks_private_ips`
- `opts_into_config_can_allow_private_ips`

Wiremock integration tests in
[`crates/tako-providers/bedrock/tests/url_prefetch_dns.rs`](crates/tako-providers/bedrock/tests/url_prefetch_dns.rs):
- `prefetch_rejects_resolved_loopback_ip_when_blocking` — bind wiremock
  on `127.0.0.1`, point a `ContentPart::ImageUrl` at it, prefetch should
  fail with `TakoError::Invalid` containing "blocked IP" and the wiremock
  server should receive ZERO requests.
- `prefetch_allows_resolved_loopback_when_allow_private_ips_set` —
  same setup with `with_url_prefetch_allow_private_ips()` flipped;
  rewrite succeeds and wiremock records one GET.

Phase 28 unit tests already in the file bind wiremock to `127.0.0.1`
through the `MockServer::start()` helper. Each one needs
`with_url_prefetch_allow_private_ips()` (or building the
`UrlPrefetchConfig` with `block_private_ips: false`) added to keep
green. Update count: ~6 tests.

## B. Ollama URL pre-fetch: same SSRF hardening

Per ARCHITECTURE.md hard rule (provider crates depend only on
`tako-core` + their vendor SDK + reqwest; never on each other), the
helpers are duplicated rather than shared. Phase 28.B already
established the duplication; Phase 29.B simply extends each copy.

Mirror of A in:
- [`crates/tako-providers/ollama/src/url_prefetch.rs`](crates/tako-providers/ollama/src/url_prefetch.rs)
- [`crates/tako-providers/ollama/src/client.rs`](crates/tako-providers/ollama/src/client.rs)

Same test surface as A (with `_ollama_` in test names where they mention
the provider). Same Phase 28 test updates needed (~6 Ollama tests).

## C. Python facade: `tako.providers.Ollama` + `block_private_ips` kwarg on Bedrock

### C.1 — `crates/tako-py/src/py_ollama.rs` (NEW)

Mirror of `py_bedrock.rs`. Pyclass:

```rust
#[pyclass(name = "Ollama", module = "tako._native", from_py_object)]
pub struct PyOllama {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyOllama {
    #[new]
    #[pyo3(signature = (
        model,
        base_url=None,
        timeout_secs=None,
        url_prefetch=false,
        url_prefetch_allow_http=false,
        url_prefetch_allow_private_ips=false,
        url_prefetch_timeout_secs=None,
        url_prefetch_max_bytes=None,
    ))]
    fn new(
        py: Python<'_>,
        model: String,
        base_url: Option<String>,
        timeout_secs: Option<u64>,
        url_prefetch: bool,
        url_prefetch_allow_http: bool,
        url_prefetch_allow_private_ips: bool,
        url_prefetch_timeout_secs: Option<u64>,
        url_prefetch_max_bytes: Option<usize>,
    ) -> PyResult<Self> {
        let mut b = OllamaProvider::builder().model(model);
        if let Some(url) = base_url { b = b.base_url(url); }
        if let Some(secs) = timeout_secs { b = b.timeout(Duration::from_secs(secs)); }
        if url_prefetch { b = b.with_url_prefetch(); }
        if url_prefetch_allow_http { b = b.with_url_prefetch_allow_http(); }
        if url_prefetch_allow_private_ips { b = b.with_url_prefetch_allow_private_ips(); }
        if let Some(secs) = url_prefetch_timeout_secs {
            b = b.with_url_prefetch_timeout(Duration::from_secs(secs));
        }
        if let Some(bytes) = url_prefetch_max_bytes {
            b = b.with_url_prefetch_max_bytes(bytes);
        }
        // OllamaBuilder::build() is sync (no async credential chain);
        // Bedrock needs py.detach + rt.block_on, Ollama doesn't.
        let provider = b.build()?;
        Ok(Self { handle: ProviderHandle::new(Arc::new(provider)) })
    }

    fn id(&self) -> &str { self.handle.inner.id() }
}
```

### C.2 — Extend `PyBedrock::new()`

Add new `url_prefetch_allow_private_ips: bool = false` kwarg between the
existing `url_prefetch_allow_http` and `url_prefetch_timeout_secs`
positions; plumb through to
`BedrockBuilder::with_url_prefetch_allow_private_ips()`.

### C.3 — Register `PyOllama`

[`crates/tako-py/src/lib.rs`](crates/tako-py/src/lib.rs):

```rust
mod py_ollama;
// ...
m.add_class::<py_ollama::PyOllama>()?;  // alongside PyBedrock
```

### C.4 — Python-side mirrors

[`python/tako/providers.py`](python/tako/providers.py): new
`class Ollama(_ProviderBase)` with the same docstring shape as the
Phase 28.C `Bedrock` class (Phase 29 SSRF mitigations summary, operator-
egress reminder). Also add `url_prefetch_allow_private_ips: bool =
False` kwarg to the `Bedrock` class.

[`python/tako/_native.pyi`](python/tako/_native.pyi): new `Ollama` stub
+ extend `Bedrock` stub with the new kwarg.

### C.5 — Tests

- [`tests/python/test_phase29_ssrf_hardening.py`](tests/python/test_phase29_ssrf_hardening.py)
  — pin the `url_prefetch_allow_private_ips` kwarg presence + default
  on both `Bedrock` and `Ollama`.
- [`tests/python/test_phase29_ollama_facade.py`](tests/python/test_phase29_ollama_facade.py)
  — pin the new `Ollama` class exists, has the expected kwargs, and
  the docstring documents the SSRF mitigation surface.

Both files use signature smoke (mirror Phase 28.C's
`test_phase28_url_prefetch.py` style) since constructing live
providers needs AWS credentials / Ollama daemon.

## Out of scope (deferred to Phase 30+)

- **Per-domain allowlist** — operators may want to permit specific
  internal hostnames while still blocking everything else. Phase 29
  ships only the binary on/off flag.
- **IPv6 documentation prefix (`2001:db8::/32`)** — currently allowed
  by the Phase 29 blocklist; defence-in-depth tightening for later.
- **OIDC mTLS end-to-end integration test** (Phase 28 carry-forward).
- **Vertex File API upload flow** (Phase 23 carry-forward).
- **`TakoError::Provider` short-circuit on `ChainedAuthResolver`**
  (Phase 27 carry-forward).

## Acceptance criteria

- `cargo test -p tako-providers-bedrock --all-features` passes
- `cargo test -p tako-providers-ollama --all-features` passes
- `cargo test -p tako-py --all-features` passes
- `cargo clippy --workspace --all-features -- -D warnings` passes
- `cargo fmt --all -- --check` passes
- `pytest -q` passes (after `maturin develop --release`)
- `python/tako/_native.pyi` has both `Bedrock` and `Ollama` stubs with
  the new `url_prefetch_allow_private_ips` kwarg
- `python/tako/providers.py` has both `Bedrock` and `Ollama` classes
  with the new kwarg

## Commit cadence

1. `docs: PLAN_PHASE29.md`
2. `feat(tako-providers/bedrock): URL pre-fetch private-IP blocklist + DNS-rebind mitigation (Phase 29.A)`
3. `feat(tako-providers/ollama): URL pre-fetch private-IP blocklist + DNS-rebind mitigation (Phase 29.B)`
4. `feat(tako-py): tako.providers.Ollama facade + Bedrock allow_private_ips kwarg (Phase 29.C)`
5. `docs: Phase 29 PLAN/README/CHANGELOG flip (v0.30.0)`
