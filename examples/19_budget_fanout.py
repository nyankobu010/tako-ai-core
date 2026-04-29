"""Phase 6.A: budget tracking across a Conductor's worker fan-out.

The coordinator dispatches two parallel workers; each worker call goes
through ``BudgetTracker::pre_check`` + ``record`` independently. After
the run the backend reports the cumulative usage for the tenant —
proving every coordinator + worker hop landed in the budget ledger.

Uses ``PythonProvider`` so the example runs with no API keys.
"""

from __future__ import annotations

import asyncio
from collections.abc import Iterator
from typing import Any

import tako


def _coord_factory(scripts: list[str]) -> Any:
    it: Iterator[str] = iter(scripts)

    async def chat(_request: dict[str, Any]) -> str:
        return next(it)

    return chat


async def _worker(role: str) -> Any:
    async def chat(_request: dict[str, Any]) -> str:
        return f"{role}-result"

    return chat


async def main() -> None:
    coord = tako.providers.PythonProvider(
        "py:coord",
        chat=_coord_factory(
            [
                '{"thought":"divide-and-conquer",'
                '"dispatch":[{"worker":"code","task":"write fib"},'
                '{"worker":"math","task":"verify"}],'
                '"halt":false}',
                '{"thought":"shipped","dispatch":[],"halt":true,"final_answer":"all done"}',
            ]
        ),
    )
    code = tako.providers.PythonProvider("py:code", chat=await _worker("code"))
    math = tako.providers.PythonProvider("py:math", chat=await _worker("math"))

    backend = tako.budget.InMemoryBackend()
    cond = tako.Conductor(
        coordinator=coord,
        workers={"code": code, "math": math},
        max_steps=4,
        budget=tako.Budget(max_usd_per_day=10.0),
        budget_backend=backend,
    )

    result = await cond.run("plan and verify", tenant_id="acme")
    print(f"final answer: {result.text!r}")

    usage = await backend.current_usage("acme")
    print(f"acme usage today: ${usage.usd_today:.4f} / {usage.tokens_today} tokens")


if __name__ == "__main__":
    asyncio.run(main())
