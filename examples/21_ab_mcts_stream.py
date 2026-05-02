"""Phase 8.B: native AB-MCTS streaming.

Drives a small AB-MCTS search using a `Fake` provider that emits a
canned response and a rule-based verifier (length-based score). The
stream emits, per iteration:

  step_start  → assistant_text  → verifier_score

then a single terminal `final` event after the search loop completes
or `min_confidence` short-circuits it.
"""

from __future__ import annotations

import asyncio

import tako


async def main() -> None:
    fake = tako.providers.Fake(canned_text="a thirty-character output here")
    verifier = tako.verifiers.RuleBased(min_chars=20)
    mcts = tako.AbMcts(
        fake,
        verifier,
        max_iterations=4,
        max_steps_per_rollout=1,
        min_confidence=0.95,
    )

    async for ev in await mcts.stream("explore the answer"):
        if ev.kind == "step_start":
            print(f"[iter {ev.step}] step_start")
        elif ev.kind == "assistant_text":
            print(f"[iter {ev.step}] rollout text: {ev.delta!r}")
        elif ev.kind == "verifier_score":
            print(f"[iter {ev.step}] branch={ev.branch} score={ev.score:.3f}")
        elif ev.kind == "final":
            print(f"[final] {ev.text!r}")
            usage = ev.usage or {}
            print(f"[final] usage={usage}")


if __name__ == "__main__":
    asyncio.run(main())
