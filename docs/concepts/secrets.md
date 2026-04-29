# Secret resolvers

`SecretResolver` is a one-method trait:

```rust
async fn resolve(&self, key: &str) -> Result<SecretString, TakoError>;
```

`SecretString` redacts itself in `Debug` and `Display` (renders
`<redacted>`); call `expose()` to read the value.

## Available resolvers

| Resolver | Auth | Notes |
|----------|------|-------|
| `EnvResolver` | env vars | Default; reads `std::env::var(key)` |
| `VaultResolver` | `X-Vault-Token` | KV-v2 REST API; `path#field` selects sub-key |
| `AwsSecretsManagerResolver` | AWS chain | Reuses `aws-config` from Bedrock |
| `AzureKeyVaultResolver` | bearer token (deferred) | REST API; `name#version` opt |
| `GcpSecretManagerResolver` | bearer token (deferred) | REST API; `name#version` opt |

## Deferred-auth pattern

Azure KV and GCP SM accept a *pre-resolved* OAuth2 access token at
construction time. The resolver does not refresh tokens — wire your own
credential source (gcloud / gcp_auth / Azure managed identity) and
rebuild the resolver before tokens expire. This keeps tako's dependency
tree small and compatible with whatever credential flow you already
have.

## Using it from Python

```python
import tako

vault = tako.secrets.VaultResolver(
    addr="http://127.0.0.1:8200",
    token=os.environ["VAULT_TOKEN"],
)
api_key = await vault.resolve("secret/data/myapp#openai_api_key")

provider = tako.providers.OpenAI(model="gpt-5", api_key=api_key)
```

## Key syntax

Most resolvers accept a `name#version` suffix:

| Resolver | Suffix means |
|----------|--------------|
| Vault | JSON-pointer field within the secret object |
| Azure KV | secret version id |
| GCP SM | numeric version (default `latest`) |
| AWS SM | version id |

`EnvResolver` ignores the suffix.

## See also

- [recipes/secret_resolvers.md](../recipes/secret_resolvers.md)
- API reference: [Python module reference](../api/python.md)
