"""Phase 3: Trinity orchestrator with rule-based router.

Routes each prompt to one of three Fake providers. Real deployments
would use real providers (OpenAI / Anthropic / Bedrock / etc.) and
optionally swap RegexRouter for OnnxRouter once a model is trained.
"""

from __future__ import annotations

import asyncio

import tako
from tako.routers import RegexRouter


async def main() -> None:
    trinity = tako.Trinity(
        roles={
            "code": tako.providers.Fake(canned_text="<<CODE>>", id="fake:code"),
            "math": tako.providers.Fake(canned_text="<<MATH>>", id="fake:math"),
            "fallback": tako.providers.Fake(canned_text="<<FB>>", id="fake:fb"),
        },
        router=RegexRouter(),
    )

    for prompt in [
        "Write a Rust fn that returns 42",
        "Solve x^2 + 5x + 6 = 0",
        "Hi! How are you?",
    ]:
        print(f"> {prompt}")
        print(f"  → {(await trinity.run(prompt)).text}")


if __name__ == "__main__":
    asyncio.run(main())
