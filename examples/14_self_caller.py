"""Phase 3: SelfCaller bounded recursion with a rule-based guard.

The wrapped SingleAgent emits a short canned answer; RuleBased
requires `min_chars >= 50`, so it fails. SelfCaller will recurse up
to `max_depth=2` more times before returning the last attempt.

In a real deployment, the inner orchestrator would produce different
text on each recursion (the revision_prompt nudges it), so the
threshold could eventually be met.
"""

from __future__ import annotations

import asyncio

import tako
from tako.guards import RuleBased


async def main() -> None:
    fake = tako.providers.Fake(canned_text="too short")
    inner = tako.SingleAgent(provider=fake, max_steps=1)

    sc = tako.SelfCaller(
        inner=inner,
        confidence=RuleBased(min_chars=50),
        max_depth=2,
        min_confidence=0.5,
    )
    result = await sc.run("Explain CRDTs in detail")
    print(f"Final answer: {result.text}")
    print(f"Inner provider was called {fake.call_count} times.")


if __name__ == "__main__":
    asyncio.run(main())
