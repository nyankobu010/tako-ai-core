# OIDC mTLS cert/key rotation

Refresh `tako-compat`'s OIDC introspection mTLS client cert + key
without restarting the process.

## When to use this

Production deployments typically have a sidecar (cert-manager, Vault
PKI) or an out-of-band watcher refreshing client certs to short
validity windows. Without rotation support, every refresh would
require a process restart. The
`OidcAuthResolver::reload_mtls_identity` primitive lets operators
swap the client identity in place.

## Wire it up

Install the initial identity at startup and call `reload_mtls_identity`
from your refresh source. Example with a filesystem-watcher pattern:

```python
import asyncio
import tako.compat

oidc = (
    tako.compat.OidcAuth(issuer="https://issuer.example.com", audience="my-api")
    .with_introspection(client_id="my-api", client_secret="")
    .with_introspection_mtls(
        cert_pem=open("/var/run/secrets/oidc-mtls.crt").read(),
        key_pem=open("/var/run/secrets/oidc-mtls.key").read(),
    )
)

async def watch_certs(oidc):
    # Replace with your real watcher (notify / inotify / cert-manager events).
    while True:
        await asyncio.sleep(60)
        cert = open("/var/run/secrets/oidc-mtls.crt").read()
        key = open("/var/run/secrets/oidc-mtls.key").read()
        try:
            oidc.reload_mtls_identity(cert, key)
        except Exception as e:
            # PEM parse / Client build failure — the previously installed
            # client is preserved (no partial-rollback).
            log.exception("mTLS reload failed: %s", e)

asyncio.create_task(watch_certs(oidc))

await tako.compat.serve_openai(
    orchestrator=orch,
    bind="0.0.0.0:8080",
    auth=oidc,
)
```

## Atomicity

The swap is atomic from the request-handler's perspective. Concurrent
introspection POSTs either see the old `reqwest::Client` or the new
one, never a torn state — the snapshot lives for the duration of one
request, and reloads affect only the *next* request after them.

## Combined-PEM convenience

If your secret store hands you cert + key concatenated in one PEM
file (a common pattern), use:

```python
oidc.reload_mtls_identity_combined(open("client-combined.pem").read())
```

## Error handling

| Condition | Behaviour |
|-----------|-----------|
| No prior `with_introspection_mtls(...)` | `TakoError::Invalid` with operator guidance pointing at the right builder. |
| PEM parse failure | `TakoError::Invalid`. Previously installed client preserved. |
| `reqwest::Client` build failure | `TakoError::Invalid`. Previously installed client preserved. |
| Successful reload | The next introspection request uses the new client. |

## See also

- [Concepts → OpenAI-compat server](../concepts/compat.md)
- [recipes/oidc_introspection.md](oidc_introspection.md)
