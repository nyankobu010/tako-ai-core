# `ChainedAuth` — composite AuthResolver

Compose multiple `AuthResolver` impls so a single `tako-compat`
deployment can accept either an OIDC bearer or a static API key.

## Pattern: OIDC bearer → static API key fallback

```python
import tako.compat

oidc = (
    tako.compat.OidcAuth(issuer="https://issuer.example.com", audience="my-api")
    .with_introspection(client_id="my-api", client_secret="...")
)

static = tako.compat.StaticTokens({
    "sk-internal-1": tako.Principal(tenant_id="internal-job-runner"),
})

chain = tako.compat.ChainedAuth().then(oidc).then(static)

await tako.compat.serve_openai(
    orchestrator=orch,
    bind="0.0.0.0:8080",
    auth=chain,
)
```

Children are tried in append order. The first child to return `Ok`
short-circuits — the second child is not called. On all-`Err`, the
last child's error propagates.

## Fail-fast on infrastructure errors

By default a child error falls through. That's correct for auth
decisions (`Invalid`, `PolicyDenied`) — the next resolver might know
the token. It's **wrong** for transport-layer failures: an unreachable
OIDC issuer would surface as a misleading `unknown bearer token` from
the StaticTokens fallback.

Two opt-in fail-fast modes are available:

```python
# Narrow: only Transport halts the chain.
chain = chain.with_short_circuit_on_transport_error()

# Broader: Transport ∪ RateLimited ∪ CircuitOpen ∪ BudgetExhausted.
chain = chain.with_short_circuit_on_infrastructure_errors()
```

Auth-decision errors (`Invalid`, `PolicyDenied`) and vendor errors
(`Provider`) still fall through — those are decisions the next resolver
might overturn or that aren't the auth surface's concern.

The two builders compose last-write-wins: calling
`with_short_circuit_on_infrastructure_errors()` after
`with_short_circuit_on_transport_error()` upgrades the policy.

## Per-child policy override

The chain-wide flag forces a single sensitivity level on every
child. Real deployments often want to mix:

- **Critical primary** (OIDC issuer) — transport / rate-limit /
  circuit failures should halt the chain so the operator gets the
  actionable diagnostic, not a misleading "unknown bearer" from a
  fallback.
- **Graceful tail** (in-process static API keys) — an in-process
  resolver functionally cannot raise `RateLimited` /
  `CircuitOpen` / `BudgetExhausted`; if it does, that's a bug,
  not a "real" infrastructure failure. Operators want the tail
  to keep serving.

`then_with_short_circuit(child, policy)` (Phase 36) appends a
child with a per-child override:

```python
oidc = ...                       # critical primary
jwt = ...                        # critical secondary
static = tako.compat.StaticTokens({"sk-dev-1": tako.Principal(...)})

chain = (
    tako.compat.ChainedAuth()
    .then(oidc)                  # Inherit chain-wide policy
    .then(jwt)                   # Inherit chain-wide policy
    .then_with_short_circuit(static, "always_fall_through")
    .with_short_circuit_on_infrastructure_errors()
)
```

Accepted policy aliases (case-insensitive; kebab-case variants
also work):

| Policy | Behaviour |
|--------|-----------|
| `"inherit"` | Same as bare `then(child)`. Default. |
| `"always_fall_through"` | Override: every error from this child falls through. Use for graceful-tail fallbacks. |
| `"transport_only"` | Override: short-circuit only on `Transport`. Narrower than chain-wide infra. |
| `"all_infrastructure"` | Override: short-circuit on `Transport` / `RateLimited` / `CircuitOpen` / `BudgetExhausted`. Broader than chain-wide transport-only. |

Override priority: when a child's policy is anything other than
`"inherit"`, that policy alone determines whether the child's
error halts the chain — the chain-wide flag is ignored for this
child. Bare `then(child)` is equivalent to
`then_with_short_circuit(child, "inherit")` byte-for-byte; you
can mix both builders in any order.

## Nesting

Chains compose recursively — a chain whose child is itself a chain
works:

```python
inner = tako.compat.ChainedAuth().then(jwt).then(static)
outer = tako.compat.ChainedAuth().then(oidc).then(inner)
```

This is how you express "try OIDC, then try (try JWT, then try static)".

## See also

- [Concepts → OpenAI-compat server](../concepts/compat.md)
- [recipes/oidc_introspection.md](oidc_introspection.md)
