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
