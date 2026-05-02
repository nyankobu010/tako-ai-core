"""Phase 10.C: Conductor emits OrchEvent::VerifierScore per worker.

When a `tako.verifiers.RuleBased` (or any `tako.verifiers.*`) is
attached via `verifier=...` on `tako.Conductor`, the streaming path
emits one `OrchEvent::VerifierScore` per worker output, with
``branch`` = the 1-based worker dispatch index within the current
coordinator turn. Failed workers are skipped — only successful text
outputs are scored.

Without ``verifier=...``, no `VerifierScore` events appear (v0.10.0
behaviour is byte-for-byte preserved).
"""

from __future__ import annotations

import asyncio
import json
from typing import Any

import tako


# Coordinator that fans out to three workers in one turn, then halts.
async def coord_chat(_request: dict[str, Any]) -> str:
    return json.dumps(
        {
            "thought": "delegating",
            "dispatch": [
                {"worker": "alpha", "task": "task A"},
                {"worker": "beta", "task": "task B"},
                {"worker": "gamma", "task": "task C"},
            ],
            "halt": False,
        }
    )


async def coord_halt(_request: dict[str, Any]) -> str:
    return json.dumps(
        {
            "thought": "all workers done",
            "dispatch": [],
            "halt": True,
            "final_answer": "done",
        }
    )


async def main() -> None:
    # Coordinator: deterministic two-call sequence (dispatch then halt)
    # via a tiny stateful closure.
    calls = {"n": 0}

    async def coord(req: dict[str, Any]) -> str:
        n = calls["n"]
        calls["n"] = n + 1
        return await (coord_chat(req) if n == 0 else coord_halt(req))

    async def worker(_request: dict[str, Any]) -> str:
        # Each worker produces enough text to pass `min_chars=4`.
        return "worker output OK"

    cond = tako.Conductor(
        coordinator=tako.providers.PythonProvider("py:coord", chat=coord),
        workers={
            "alpha": tako.providers.PythonProvider("py:alpha", chat=worker),
            "beta": tako.providers.PythonProvider("py:beta", chat=worker),
            "gamma": tako.providers.PythonProvider("py:gamma", chat=worker),
        },
        max_steps=5,
        verifier=tako.verifiers.RuleBased(min_chars=4),
    )

    out = await cond.run("plan three workers")
    print(f"final: {out.text!r}")
    # Note: Conductor doesn't expose `.stream()` on the Python facade in
    # v0.11.0 (only SelfCaller / AbMcts do); the VerifierScore events
    # are visible from a Rust-side `.stream()` call. The Rust integration
    # test `crates/tako-orchestrator/tests/conductor.rs::verifier_emits`
    # asserts `branch ∈ {1, 2, 3}` for the three workers above.


if __name__ == "__main__":
    asyncio.run(main())
