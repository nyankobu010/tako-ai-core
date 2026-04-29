"""Cloud secret resolvers.

Each resolver exposes ``async resolve(key: str) -> str``. Keys may be
plain secret names (or paths, depending on the backend) and may include
an optional ``#suffix`` to disambiguate sub-keys (Vault), versions
(Azure KV, GCP SM, AWS SM).

The returned string is the raw secret value — handle it like a password:
do not log it, prefer passing it directly to the consuming API client,
and avoid keeping it resident longer than necessary.
"""

from __future__ import annotations

from typing import Any

from tako import _native


class _ResolverBase:
    _handle: Any

    async def resolve(self, key: str) -> str:
        return str(await self._handle.resolve(key))


class VaultResolver(_ResolverBase):
    """HashiCorp Vault KV-v2 resolver.

    ``addr`` is the Vault server URL (e.g.
    ``https://vault.example:8200``). ``token`` is sent as the
    ``X-Vault-Token`` header. Keys must include the KV-v2 ``data/``
    segment, e.g. ``"secret/data/myapp"``; append ``#field`` to extract a
    single key from the secret object.
    """

    def __init__(self, addr: str, token: str) -> None:
        self._handle = _native.VaultResolver(addr, token)


class AzureKeyVaultResolver(_ResolverBase):
    """Azure Key Vault REST resolver.

    ``vault_url`` is the Key Vault DNS name (e.g.
    ``https://my-vault.vault.azure.net``). ``access_token`` is a
    pre-resolved Azure AD bearer token scoped to
    ``https://vault.azure.net/.default`` — the resolver does not refresh
    it. Keys may be ``"<secret-name>"`` (latest version) or
    ``"<secret-name>#<version-id>"``.
    """

    def __init__(
        self,
        vault_url: str,
        access_token: str,
        *,
        api_version: str | None = None,
    ) -> None:
        self._handle = _native.AzureKeyVaultResolver(
            vault_url,
            access_token,
            api_version=api_version,
        )


class GcpSecretManagerResolver(_ResolverBase):
    """GCP Secret Manager REST resolver.

    Reads via ``:access`` against the Secret Manager v1 API. Like the
    Vertex provider, OAuth2 token acquisition is deferred — pass a
    pre-resolved access token (``gcloud auth print-access-token`` or the
    GKE metadata server). Keys may be ``"<secret-name>"`` (latest
    version) or ``"<secret-name>#<version-number>"``.
    """

    def __init__(
        self,
        project_id: str,
        access_token: str,
        *,
        endpoint_url: str | None = None,
    ) -> None:
        self._handle = _native.GcpSecretManagerResolver(
            project_id,
            access_token,
            endpoint_url=endpoint_url,
        )


class AwsSecretsManagerResolver(_ResolverBase):
    """AWS Secrets Manager resolver.

    Uses the AWS SDK with the standard credential chain (env vars,
    profile, IRSA, IMDS). Credential resolution happens on the first
    ``resolve()`` call so the constructor cannot fail in test
    environments without AWS creds.
    """

    def __init__(
        self,
        *,
        region: str | None = None,
        profile_name: str | None = None,
        endpoint_url: str | None = None,
    ) -> None:
        self._handle = _native.AwsSecretsManagerResolver(
            region=region,
            profile_name=profile_name,
            endpoint_url=endpoint_url,
        )


__all__ = [
    "AwsSecretsManagerResolver",
    "AzureKeyVaultResolver",
    "GcpSecretManagerResolver",
    "VaultResolver",
]
