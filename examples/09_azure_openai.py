"""Phase 2.5 example: SingleAgent backed by Azure OpenAI.

Azure OpenAI uses the same wire format as OpenAI's chat.completions, but
routes by *deployment name* (a user-defined alias mapping to a model)
rather than model id, and uses an ``api-key`` header instead of bearer
auth.

Run:
    pip install tako
    export AZURE_OPENAI_ENDPOINT="https://my-resource.openai.azure.com"
    export AZURE_OPENAI_DEPLOYMENT="gpt-4o-prod"
    export AZURE_OPENAI_API_KEY="..."
    python examples/09_azure_openai.py
"""

from __future__ import annotations

import asyncio
import os

import tako


async def main() -> None:
    endpoint = os.getenv("AZURE_OPENAI_ENDPOINT")
    deployment = os.getenv("AZURE_OPENAI_DEPLOYMENT")
    api_key = os.getenv("AZURE_OPENAI_API_KEY")

    if not (endpoint and deployment and api_key):
        # Skip live API call when env vars are missing; run against the
        # in-process Fake provider so CI can still execute the file.
        provider: tako.providers._ProviderBase = tako.providers.Fake(
            canned_text="(skipped: AZURE_OPENAI_* env vars not set)",
        )
    else:
        provider = tako.providers.AzureOpenAI(
            endpoint=endpoint,
            deployment=deployment,
            api_key=api_key,
            # api_version defaults to 2024-10-21; override for previews:
            # api_version="2025-01-01-preview",
        )

    agent = tako.SingleAgent(provider=provider, max_steps=4)
    result = await agent.run(
        "In one sentence: what is an octopus?",
        tenant_id="demo",
        user_id="alice",
    )
    print(f"[{provider.id}] {result.text}")


if __name__ == "__main__":
    asyncio.run(main())
