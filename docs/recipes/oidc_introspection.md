# OIDC introspection on tako-compat

Configure `tako-compat`'s `OidcAuthResolver` to validate bearer tokens
against an OIDC issuer's RFC 7662 introspection endpoint.

## Minimal: discovery + JWKS only

```python
import tako.compat

oidc = tako.compat.OidcAuth(
    issuer="https://issuer.example.com",
    audience="my-api",
)

await tako.compat.serve_openai(
    orchestrator=orch,
    bind="0.0.0.0:8080",
    auth=oidc,
)
```

This validates incoming `Bearer ...` JWTs against the issuer's JWKS
(rotation-aware: a one-shot force-refresh fires on the first signature
failure to handle key roll within the cache window).

## Add introspection (RFC 7662)

```python
oidc = (
    tako.compat.OidcAuth(issuer="https://issuer.example.com", audience="my-api")
    .with_introspection(client_id="my-api", client_secret="...")
    # let discovery pick the strongest mutually-supported auth method:
    .with_introspection_auth_method_from_discovery()
)
```

The auto-selector reads the issuer's
`introspection_endpoint_auth_methods_supported` discovery list and
picks (preference order):

1. `tls_client_auth` (CA-backed mTLS, RFC 8705)
2. `self_signed_tls_client_auth` (RFC 8705 §2.2)
3. `private_key_jwt` (asymmetric JWT, RFC 7521 / 7523)
4. `client_secret_jwt` (HS256 JWT, RFC 7521 / 7523)
5. `client_secret_basic` (HTTP Basic header)
6. `client_secret_post` (form body)

Pin a specific method explicitly when the auto-selector's choice isn't
what you want:

```python
oidc = oidc.with_introspection_auth_method("client_secret_jwt")
```

(Aliases are accepted: `"jwt"`, `"basic"`, `"post"`, `"private_key_jwt"`,
`"tls_client_auth"`/`"mtls"`, `"self_signed_tls_client_auth"`/`"self_signed_mtls"`,
case-insensitive.)

## `private_key_jwt` (RS256 / ES256 / EdDSA)

```python
oidc = oidc.with_introspection_jwt_rs256_pem(
    rs256_private_key_pem=open("client.pem").read(),
)
# or .with_introspection_jwt_es256_pem(...)
# or .with_introspection_jwt_ed25519_pem(...)
```

Each helper loads the PEM **and** flips the auth method to
`PrivateKeyJwt` in one call.

## mTLS (`tls_client_auth`)

```python
oidc = oidc.with_introspection_mtls(
    cert_pem=open("client.crt").read(),
    key_pem=open("client.key").read(),
)
# convenience: pass a single combined PEM
# oidc = oidc.with_introspection_mtls_combined(open("client-combined.pem").read())
```

## Live cert/key rotation

For deployments where cert-manager / Vault PKI / a filesystem watcher
refreshes mTLS certs without process restart, install the initial
identity at startup and call `reload_mtls_identity` from your scheduler:

```python
import asyncio

async def cert_rotation_loop(oidc, watcher):
    async for new_cert_pem, new_key_pem in watcher:
        oidc.reload_mtls_identity(new_cert_pem, new_key_pem)
```

The swap is atomic: concurrent introspection POSTs see either the old
client or the new one, never a torn state. Reloading without a prior
`with_introspection_mtls` raises `TakoError::Invalid` (operator notices
early; not a silent no-op).

## End-session helper

```python
logout_uri = oidc.build_logout_uri(
    id_token_hint=user_id_token,
    post_logout_redirect_uri="https://app.example.com/post-logout",
    state="opaque-state-from-session",
)
return RedirectResponse(logout_uri)
```

## See also

- [Concepts → OpenAI-compat server](../concepts/compat.md)
- [recipes/chained_auth.md](chained_auth.md)
- [recipes/mtls_rotation.md](mtls_rotation.md)
