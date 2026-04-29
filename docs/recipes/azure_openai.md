# Recipe: Azure OpenAI

Azure OpenAI uses the same chat.completions wire format as OpenAI, but
routes by *deployment name* (a user-defined alias mapping to a model)
and uses an `api-key` header instead of bearer auth.

```python
import os
import tako

provider = tako.providers.AzureOpenAI(
    endpoint=os.environ["AZURE_OPENAI_ENDPOINT"],     # https://my-resource.openai.azure.com
    deployment=os.environ["AZURE_OPENAI_DEPLOYMENT"], # e.g. "gpt-4o-prod"
    api_key=os.environ["AZURE_OPENAI_API_KEY"],
    # api_version defaults to 2024-10-21; override for previews:
    api_version="2025-01-01-preview",
)

agent = tako.SingleAgent(provider=provider, max_steps=4)
result = await agent.run("In one sentence: what is an octopus?")
print(result.text)
```

The provider id surfaces as `azure-openai:<deployment>`. Streaming and
tool calls follow the same code paths as the OpenAI provider — the only
behavioral difference is the URL shape and auth header.

## Pulling the API key from Vault

```python
vault = tako.secrets.VaultResolver(
    addr=os.environ["VAULT_ADDR"],
    token=os.environ["VAULT_TOKEN"],
)
api_key = await vault.resolve("secret/data/azure-openai#api_key")

provider = tako.providers.AzureOpenAI(
    endpoint=os.environ["AZURE_OPENAI_ENDPOINT"],
    deployment="gpt-4o-prod",
    api_key=api_key,
)
```

## Multi-region failover

Azure OpenAI deployments are per-region. To fail over across regions,
build two providers and put them behind a `FallbackProvider`:

```python
primary = tako.providers.AzureOpenAI(
    endpoint="https://eastus.openai.azure.com",
    deployment="gpt-4o",
    api_key=key,
)
secondary = tako.providers.AzureOpenAI(
    endpoint="https://westus.openai.azure.com",
    deployment="gpt-4o",
    api_key=key,
)
client = tako.Client(providers=[primary, secondary])
```

The runtime cascades through providers when the primary returns a
transient error or trips its circuit breaker.
