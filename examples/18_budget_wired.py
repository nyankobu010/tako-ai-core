"""Phase 5.C: a SingleAgent run that consults a budget backend.

Wires :class:`tako.budget.InMemoryBackend` into :class:`tako.SingleAgent`
via the new ``budget=`` / ``budget_backend=`` kwargs. After the run the
backend reports cumulative usage for the tenant, demonstrating that
``BudgetTracker.record`` fired on every provider call.

For multi-process deployments swap the backend with
:class:`tako.budget.RedisBackend` — see ``examples/18_budget_redis.py``
(skipped here when ``REDIS_URL`` is unset).
"""

from __future__ import annotations

import asyncio

import tako


async def main() -> None:
    fake = tako.providers.Fake(canned_text="weather is sunny")
    backend = tako.budget.InMemoryBackend()
    agent = tako.SingleAgent(
        provider=fake,
        max_steps=1,
        budget=tako.Budget(max_usd_per_request=5.0, max_usd_per_day=100.0),
        budget_backend=backend,
    )

    result = await agent.run("What's the weather?", tenant_id="acme")
    print(f"agent: {result.text!r}")

    usage = await backend.current_usage("acme")
    print(f"acme usage today: ${usage.usd_today:.4f} / {usage.tokens_today} tokens")


if __name__ == "__main__":
    asyncio.run(main())
