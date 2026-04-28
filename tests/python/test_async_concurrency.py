"""Prove the GIL is actually released during async runs.

Strategy: run 10 orchestrator invocations concurrently against a
FakeProvider that sleeps 50ms per call. If we held the GIL across the
await points, total wall-clock time would be ~10 * 50 = 500 ms. With
proper py.detach + future_into_py, the calls overlap and the wall
clock should be well under 1.5 * 50 = 75 ms.

We use a generous 250ms budget to absorb CI noise but still fail loudly
if the GIL leaks.
"""

from __future__ import annotations

import asyncio
import time

import tako


async def test_concurrent_runs_do_not_serialise() -> None:
    fake = tako.providers.Fake(canned_text="hi", delay_ms=50)
    agent = tako.SingleAgent(provider=fake)

    start = time.perf_counter()
    results = await asyncio.gather(*[agent.run(f"q{i}") for i in range(10)])
    elapsed_ms = (time.perf_counter() - start) * 1000

    assert len(results) == 10
    assert all(r.text == "hi" for r in results)
    assert fake.call_count == 10
    # Allowance: 50ms + scheduling overhead. Budget is 5x single-call
    # latency; if we serialised, we'd be at ~500ms.
    assert elapsed_ms < 250, (
        f"async runs serialised: {elapsed_ms:.1f}ms for 10 concurrent calls "
        f"(expected ~50ms; > 250ms means the GIL isn't being released)"
    )
