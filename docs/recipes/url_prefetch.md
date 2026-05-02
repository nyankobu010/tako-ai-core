# URL pre-fetch (Bedrock + Ollama)

Configure the opt-in tako-side URL fetcher so URL-source images work on
Bedrock and Ollama. See
[Concepts → URL pre-fetch & SSRF](../concepts/url_prefetch.md) for the
"why".

## Minimal

```python
provider = tako.providers.Bedrock(
    region="us-east-1",
    model_id="anthropic.claude-3-5-sonnet-20241022-v2:0",
    url_prefetch=True,
)
```

This enables `https`-only fetches with the default 10 s timeout, 10 MiB
size cap, MIME validation, and the default-on private-IP blocklist
(loopback, RFC 1918, link-local, multicast, IPv6 unique-local).

## Configure caps

```python
provider = tako.providers.Ollama(
    base_url="http://127.0.0.1:11434",
    url_prefetch=True,
    url_prefetch_timeout_secs=5,
    url_prefetch_max_bytes=2 * 1024 * 1024,
)
```

## Allowlist a private artifact registry

The default blocklist rejects RFC 1918 addresses, which is correct
behaviour for the public internet but wrong for an internal artifact
registry. The allowlist supports three forms:

```python
provider = tako.providers.Bedrock(
    region="us-east-1",
    model_id="...",
    url_prefetch=True,
    # Exact-string match on the URL host.
    url_prefetch_allow_hosts=["registry.corp"],
    # ...or wildcard suffix (matches multi-level subdomains too).
    # url_prefetch_allow_hosts=["*.internal.corp"],
    # ...or a CIDR subnet (covers many dynamic hosts under one rule).
    url_prefetch_allow_cidrs=["10.0.5.0/24"],
)
```

Allowlisted hostnames bypass **only** the private-IP blocklist for that
host; scheme / timeout / size cap / MIME validation still apply.

## Big-hammer override

For deployments where the network layer enforces egress filtering
(VPC egress rules, Pod-level egress NetworkPolicies):

```python
provider = tako.providers.Bedrock(
    region="us-east-1",
    model_id="...",
    url_prefetch=True,
    url_prefetch_allow_private_ips=True,  # disables the whole blocklist
)
```

Prefer the per-host allowlist forms above unless you're sure the
network layer already filters egress.

## Verifying the guard

```python
import asyncio, tako

provider = tako.providers.Bedrock(
    region="us-east-1",
    model_id="...",
    url_prefetch=True,
)

req = tako.ChatRequest(messages=[
    tako.Message(role=tako.Role.User, content=[
        tako.ContentPart.text("What is at this URL?"),
        # Cloud-instance metadata endpoint — should be rejected.
        tako.ContentPart.image_url(url="https://169.254.169.254/latest/meta-data/"),
    ]),
])

try:
    await provider.chat(tako.Principal(tenant_id="test"), req)
except tako.TakoError as e:
    assert "blocked private IP" in str(e)
    print("OK — blocklist active")
```

## See also

- [Concepts → URL pre-fetch & SSRF](../concepts/url_prefetch.md)
- [Concepts → Vision content](../concepts/vision.md)
