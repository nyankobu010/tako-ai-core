# URL pre-fetch & SSRF mitigation

Bedrock and Ollama can't accept image URLs in their wire format — both
require inline bytes. To support `ContentPart::ImageUrl` on those two
providers, `tako` will fetch the URL locally and rewrite the part to
inline base64 before the call. This page describes the opt-in surface,
the SSRF mitigation stack, and the operator-grade allowlist override.

## Off by default

URL pre-fetch is **opt-in and default-off**:

```python
provider = tako.providers.Bedrock(
    region="us-east-1",
    model_id="anthropic.claude-3-5-sonnet-20241022-v2:0",
    url_prefetch=True,
    url_prefetch_timeout_secs=10,
    url_prefetch_max_bytes=10 * 1024 * 1024,
)
```

The same kwargs are mirrored on `tako.providers.Ollama(...)`.

## What's enforced when on

Defence-in-depth at every layer:

1. **`https`-only.** Plain `http://` URLs are rejected. Operators can
   override with `url_prefetch_allow_http=True` for trusted internal
   networks.
2. **Timeout cap.** Default 10 s, configurable.
3. **Size cap.** Default 10 MiB. Checked twice — once via a
   `Content-Length` pre-flight, again via a post-fetch byte counter
   (defence-in-depth against lying servers).
4. **MIME validation.** Accepts only the four supported image types
   (`image/jpeg`, `image/png`, `image/gif`, `image/webp`).
5. **Private-IP blocklist.** Default-on. Rejects:
   - Loopback (`127.0.0.0/8`, `::1`)
   - RFC 1918 (`10/8`, `172.16/12`, `192.168/16`)
   - Link-local (`169.254/16`, `fe80::/10`) — covers cloud-instance
     metadata at `169.254.169.254`
   - Unspecified, multicast, reserved (`224/4`, `240/4`, `ff00::/8`)
   - IPv6 unique-local (`fc00::/7`)
   - IPv4-mapped variants (`::ffff:127.0.0.1` etc)
6. **DNS-rebinding mitigation.** A custom `reqwest::dns::Resolve`
   implementation validates **every** returned IP — there is no
   second resolution between validation and connection. Inline
   IP-literal URLs are checked separately because reqwest skips the
   DNS resolver for IP literals.

## Operator allowlist override

The blocklist can be selectively bypassed per-host without disabling the
whole guard. Three semantic forms are supported:

| Form | Example | Matches |
|------|---------|---------|
| Exact host string | `"registry.corp"` | URL host equals the string |
| Wildcard suffix | `"*.internal.corp"` | URL host ends with `.internal.corp` (multi-level — matches both `a.internal.corp` and `b.a.internal.corp`) |
| CIDR subnet | `"10.0.5.0/24"` | Resolved IP (or IP-literal URL) falls inside the subnet (IPv4 + IPv6 both supported) |

```python
provider = tako.providers.Ollama(
    base_url="http://127.0.0.1:11434",
    url_prefetch=True,
    url_prefetch_allow_hosts=["registry.corp", "*.internal.corp"],
    url_prefetch_allow_cidrs=["10.0.5.0/24", "fc00::/7"],
)
```

Allowlisted hostnames bypass **only** the private-IP blocklist for that
host; the scheme / timeout / size cap / MIME validation still apply.

## Big-hammer override

For deployments where the network layer already filters egress (VPC
egress rules, Pod-level egress NetworkPolicies), the entire blocklist
can be disabled with:

```python
provider = tako.providers.Bedrock(
    ...,
    url_prefetch_allow_private_ips=True,
)
```

This is a sledgehammer — prefer the allowlist forms above for
production. The flag is documented because *some* operator deployments
genuinely don't need any tako-side guard.

## Where it lives

The implementation lives in [`crates/tako-providers/bedrock/src/url_prefetch.rs`](https://github.com/nyankobu010/tako-ai-core/blob/main/crates/tako-providers/bedrock/src/url_prefetch.rs)
and [`crates/tako-providers/ollama/src/url_prefetch.rs`](https://github.com/nyankobu010/tako-ai-core/blob/main/crates/tako-providers/ollama/src/url_prefetch.rs).
The two copies are intentional — provider crates depend only on
`tako-core` and their vendor SDK, never on each other (see
[`ARCHITECTURE.md`](https://github.com/nyankobu010/tako-ai-core/blob/main/ARCHITECTURE.md)).

## See also

- [Vision content](vision.md) — `ContentPart::Image` / `ContentPart::ImageUrl`.
- [recipes/url_prefetch.md](../recipes/url_prefetch.md) — full configuration walkthrough.
