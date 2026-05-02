# Recipe: secret resolvers

Move API keys out of env vars and into Vault, AWS Secrets Manager,
Azure Key Vault, or GCP Secret Manager.

## Vault (KV-v2)

```python
import os
import tako

vault = tako.secrets.VaultResolver(
    addr=os.environ["VAULT_ADDR"],
    token=os.environ["VAULT_TOKEN"],
)

# Pull a single field from a KV-v2 secret object:
api_key = await vault.resolve("secret/data/myapp#openai_api_key")

provider = tako.providers.OpenAI(model="gpt-5", api_key=api_key)
```

## AWS Secrets Manager

Reuses the AWS default credential chain (env, profile, IRSA, IMDS):

```python
aws = tako.secrets.AwsSecretsManagerResolver(region="us-east-1")
api_key = await aws.resolve("prod/openai")          # latest version
api_key = await aws.resolve("prod/openai#abc123")  # specific version
```

Pin a profile or override the endpoint for VPC-private access:

```python
aws = tako.secrets.AwsSecretsManagerResolver(
    profile_name="prod",
    endpoint_url="https://secretsmanager-vpce-foo.us-east-1.amazonaws.com",
)
```

## Azure Key Vault

Auth deferred — supply a pre-resolved bearer token. For local dev:

```bash
export AZURE_KV_TOKEN="$(az account get-access-token \
    --resource https://vault.azure.net --query accessToken -o tsv)"
```

```python
azure = tako.secrets.AzureKeyVaultResolver(
    "https://my-vault.vault.azure.net",
    os.environ["AZURE_KV_TOKEN"],
)
api_key = await azure.resolve("openai-api-key")          # latest
api_key = await azure.resolve("openai-api-key#abc123")  # version
```

For production, wire `azure_identity::DefaultAzureCredential` (or your
own token-acquisition flow) and rebuild the resolver before tokens
expire.

## GCP Secret Manager

Same deferred-auth pattern as Vertex. Locally:

```bash
export GCP_TOKEN="$(gcloud auth print-access-token)"
```

```python
gcp = tako.secrets.GcpSecretManagerResolver(
    "my-gcp-project",
    os.environ["GCP_TOKEN"],
)
api_key = await gcp.resolve("openai-api-key")    # latest
api_key = await gcp.resolve("openai-api-key#3")  # specific version number
```

The provider returns the value as a UTF-8 string, base64-decoding the
SecretManager payload automatically.

## Mixing resolvers

You can layer resolvers — for example, fall back from Vault to env vars
during local dev:

```python
async def resolve_with_fallback(key: str) -> str:
    try:
        return await vault.resolve(key)
    except ValueError:
        # Fall back to env var
        return os.environ[key]
```

There's no built-in "chain" resolver yet — Phase 3 will add one if
demand surfaces.
