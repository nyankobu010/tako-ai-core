"""Phase 3: Eval harness over the in-tree synthetic dataset.

Runs 10 tasks against a Fake provider whose canned response happens
to contain every expected token, so pass-rate is 100%. The point is
the wire-format demonstration; replace ``fake`` with a real provider
and pass-rate becomes meaningful.
"""

from __future__ import annotations

import asyncio
import json

import tako
from tako.eval import Eval, load_synthetic


async def main() -> None:
    canned = "4 42 25 paris earth 1969 def fn ok hello"
    fake = tako.providers.Fake(canned_text=canned, id="fake:eval")
    orch = tako.SingleAgent(provider=fake, max_steps=1)
    dataset = load_synthetic()

    report = await Eval(orch=orch, dataset=dataset, k=1, concurrency=4).run()
    print(json.dumps(report.model_dump(), indent=2, default=str))


if __name__ == "__main__":
    asyncio.run(main())
