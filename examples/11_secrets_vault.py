"""Phase 2.5 example: load an API key from HashiCorp Vault, then use it.

Run:
    pip install tako
    export VAULT_ADDR="http://127.0.0.1:8200"
    export VAULT_TOKEN="..."
    python examples/11_secrets_vault.py
"""

from __future__ import annotations

import asyncio
import os

import tako


async def main() -> None:
    addr = os.getenv("VAULT_ADDR")
    token = os.getenv("VAULT_TOKEN")
    if not (addr and token):
        print("(skipped: VAULT_ADDR / VAULT_TOKEN not set)")
        return

    vault = tako.secrets.VaultResolver(addr, token)

    # Pull only what we need: the OpenAI API key, stored at
    # `secret/data/myapp` with key `openai_api_key`.
    api_key = await vault.resolve("secret/data/myapp#openai_api_key")

    provider = tako.providers.OpenAI(model="gpt-5", api_key=api_key)
    agent = tako.SingleAgent(provider=provider)
    result = await agent.run("In one sentence: what is an octopus?")
    print(result.text)


if __name__ == "__main__":
    asyncio.run(main())
