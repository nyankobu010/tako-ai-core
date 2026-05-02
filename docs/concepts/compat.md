# OpenAI-compatible HTTP server

`tako-compat` is a drop-in replacement for OpenAI's
`/v1/chat/completions` endpoint. It speaks the same wire shape, so any
SDK that targets OpenAI (Python `openai`, JS `openai`, LangChain,
LiteLLM, third-party tools) can point at it unchanged. On top of the
OpenAI surface, `tako-compat` re-emits `tako`'s richer orchestrator
events as `tako.*` SSE extension events so streaming consumers can
observe verifier scores, recursion depths, and tool-call lifecycles.

## Run it

```python
import tako
import tako.compat

orch = tako.SingleAgent(
    provider=tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."),
    max_steps=10,
)

# Async server (uvicorn under the hood):
await tako.compat.serve_openai(
    orchestrator=orch,
    bind="0.0.0.0:8080",
    auth=tako.compat.StaticTokens({"sk-mykey": tako.Principal(tenant_id="acme")}),
)
```

## SSE extensions

When the client requests streaming (`stream: true`), `tako-compat`
emits the OpenAI-shaped `chat.completion.chunk` deltas plus four
`tako.*` named SSE event types:

| Event | Payload | When |
|-------|---------|------|
| `tako.verifier_score` | `{step, branch, score}` | per-delta from `Verifier::evaluate_streaming` (Trinity, Conductor, AbMcts) |
| `tako.recursion` | `{depth, confidence}` | `SelfCaller` step boundaries |
| `tako.tool_call_start` | `{id, name, args}` | tool dispatch begins |
| `tako.tool_call_result` | `{id, name, result}` | tool returns |

OpenAI clients ignore unknown named events, so existing tooling keeps
working. Clients that *do* want the richer signal (e.g. tako's own
Python SSE client) parse the named events.

## AuthResolver

The `auth=` parameter accepts any implementation of the public
`AuthResolver` async trait. Five impls ship in the workspace:

| Resolver | Cargo feature | Use case |
|----------|---------------|----------|
| `StaticTokens` | always-on | Map of `bearer-token → Principal` |
| `JwtAuthResolver` | `jwt` | HS256 / RS256 / ES256 — algorithm pinned at construction so `alg=none` and HS/RS confusion fail closed |
| `OidcAuthResolver` | `oidc` | Discovery + JWKS rotation; introspection per RFC 7662 / 8414 / 8705; mTLS with explicit `reload_mtls_identity()` swap |
| `VaultAuthResolver` | `vault` | KV v2 lookups; AppRole / Kubernetes token rotation; Enterprise namespace |
| `ChainedAuthResolver` | always-on | Composite — try children in order, first `Ok` wins |

Python facade mirrors at `tako.compat.{StaticTokens, JwtAuth, OidcAuth, VaultAuth, ChainedAuth}`.
The wheel-side feature gates are `auth-jwt` / `auth-oidc` / `auth-vault`.

## Composite auth (`ChainedAuthResolver`)

```python
chain = (
    tako.compat.ChainedAuth()
    .then(oidc_resolver)
    .then(static_tokens)
    .with_short_circuit_on_infrastructure_errors()
)
```

The chain tries children in append order; the first `Ok` wins. By
default a child error falls through to the next child — so an
unreachable OIDC issuer doesn't strand a static-key client. Two opt-in
fail-fast modes are available:

- `with_short_circuit_on_transport_error()` — `Transport` errors halt
  the chain (so an issuer-down surfaces as
  `transport error: oidc unreachable` instead of a misleading
  `unknown bearer token` from the StaticTokens fallback).
- `with_short_circuit_on_infrastructure_errors()` — broader; covers
  `Transport ∪ RateLimited ∪ CircuitOpen ∪ BudgetExhausted`. Auth
  decisions (`Invalid` / `PolicyDenied`) and vendor errors (`Provider`)
  still fall through.

## OIDC introspection

`OidcAuthResolver` ships every RFC 7662 §2.1 / RFC 8414 / RFC 8705
introspection auth method:

| Method | Wire shape |
|--------|-----------|
| `client_secret_basic` | HTTP Basic header |
| `client_secret_post` | `client_id` / `client_secret` in form body |
| `client_secret_jwt` | HS256 client-assertion (RFC 7521 / 7523) |
| `private_key_jwt` | Asymmetric (RS256 / ES256 / EdDSA) client-assertion (RFC 7521 / 7523) |
| `tls_client_auth` | CA-backed mTLS handshake (RFC 8705) |
| `self_signed_tls_client_auth` | Pre-registered cert thumbprint mTLS (RFC 8705 §2.2) |

The auto-selector
(`with_introspection_auth_method_from_discovery()`) reads the
issuer's RFC 8414 `introspection_endpoint_auth_methods_supported`
list and picks the strongest mutually-supported method (preference:
`tls_client_auth` > `self_signed_tls_client_auth` > `private_key_jwt`
> `client_secret_jwt` > `client_secret_basic` > `client_secret_post`).
Fail-closed when discovery advertises only methods you haven't
configured.

For long-running deployments where cert-manager / Vault PKI / a
filesystem watcher refreshes mTLS client certs without process restart,
operators call `OidcAuthResolver::reload_mtls_identity(cert_pem,
key_pem)` (or `reload_mtls_identity_combined`) from their own
scheduler. The swap is atomic from the request-handler's perspective —
concurrent introspection POSTs see either the old client or the new
one, never a torn state.

## End-session helper

`OidcAuthResolver::end_session_endpoint()` exposes the
issuer-advertised `end_session_endpoint`; `build_logout_uri(id_token,
post_logout_redirect_uri, state)` produces an RFC 3986-correct logout
URL per OIDC Session Management 1.0 §5. Pure URL building — no I/O.

## See also

- [Streaming](streaming.md) — `OrchEvent` shapes that map to SSE.
- [recipes/openai_compat_server.md](../recipes/openai_compat_server.md)
- [recipes/oidc_introspection.md](../recipes/oidc_introspection.md)
- [recipes/chained_auth.md](../recipes/chained_auth.md)
- [recipes/mtls_rotation.md](../recipes/mtls_rotation.md)
