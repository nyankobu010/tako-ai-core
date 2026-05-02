"""Phase 2.5 example: streaming SingleAgent backed by Amazon Bedrock.

Bedrock is wired through the AWS default credential chain (env vars,
profile, IRSA, IMDS). Run:

    aws configure  # one-time
    python examples/12_bedrock_streaming.py
"""

from __future__ import annotations

import asyncio
import os

import tako


async def main() -> None:
    if not os.getenv("AWS_REGION") and not os.getenv("AWS_PROFILE"):
        print("(skipped: no AWS_REGION / AWS_PROFILE set)")
        return

    provider = tako.providers.Bedrock(
        model="anthropic.claude-3-5-sonnet-20240620-v1:0",
        region=os.getenv("AWS_REGION", "us-east-1"),
    )

    agent = tako.SingleAgent(provider=provider, max_steps=4)
    result = await agent.run("In one sentence: what is an octopus?")
    print(f"[{provider.id}] {result.text}")


if __name__ == "__main__":
    asyncio.run(main())
