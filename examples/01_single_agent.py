"""Phase 1 example: single-agent orchestrator backed by either an
Anthropic or OpenAI provider, or a Fake provider when no API key is set.

Run:
    pip install tako
    export ANTHROPIC_API_KEY=...   # optional
    python examples/01_single_agent.py
"""

from __future__ import annotations

import asyncio
import os

import tako


async def main() -> None:
    if anthropic_key := os.getenv("ANTHROPIC_API_KEY"):
        provider = tako.providers.Anthropic(model="claude-opus-4-7", api_key=anthropic_key)
        prompt = "In one sentence: what is an octopus?"
    elif openai_key := os.getenv("OPENAI_API_KEY"):
        provider = tako.providers.OpenAI(model="gpt-5", api_key=openai_key)
        prompt = "In one sentence: what is an octopus?"
    else:
        # No keys — fall back to the in-process FakeProvider so the example
        # still runs end-to-end during CI smoke tests.
        provider = tako.providers.Fake(canned_text="An octopus is a sea creature.")
        prompt = "ignored — Fake provider returns canned text"

    agent = tako.SingleAgent(provider=provider, max_steps=4)
    result = await agent.run(prompt, tenant_id="demo", user_id="alice")
    print(result.text)


if __name__ == "__main__":
    asyncio.run(main())
