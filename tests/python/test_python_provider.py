"""``tako.providers.PythonProvider`` end-to-end tests.

Verifies the GIL-correct hand-off between Rust and a user-defined async
``chat`` callable: Python coroutine awaited from Rust, response converted
back, and concurrent invocations don't serialise.
"""

from __future__ import annotations

import asyncio
import time
from typing import Any

import tako


async def test_python_provider_string_response() -> None:
    async def chat(request: dict[str, Any]) -> str:
        msg = request["messages"][-1]
        text = msg["content"][0]["text"]
        return f"echo: {text}"

    provider = tako.providers.PythonProvider("custom:echo", chat=chat)
    agent = tako.SingleAgent(provider=provider)
    result = await agent.run("hello")
    assert result.text == "echo: hello"


async def test_python_provider_dict_response_with_usage() -> None:
    async def chat(_request: dict[str, Any]) -> dict[str, Any]:
        return {"text": "with-usage", "input_tokens": 7, "output_tokens": 3}

    provider = tako.providers.PythonProvider("custom:usage", chat=chat)
    agent = tako.SingleAgent(provider=provider)
    result = await agent.run("anything")
    assert result.text == "with-usage"


async def test_python_provider_concurrent_runs_do_not_serialise() -> None:
    """50ms asyncio.sleep, 5 concurrent — must finish well under 250ms.

    If the GIL leaked across the Rust ↔ Python ↔ Rust hand-off the calls
    would run sequentially (~250ms) instead of overlapping (~50ms).
    """

    async def chat(_request: dict[str, Any]) -> str:
        await asyncio.sleep(0.05)
        return "ok"

    provider = tako.providers.PythonProvider("custom:slow", chat=chat)
    agent = tako.SingleAgent(provider=provider)

    start = time.perf_counter()
    results = await asyncio.gather(*[agent.run(f"q{i}") for i in range(5)])
    elapsed_ms = (time.perf_counter() - start) * 1000

    assert all(r.text == "ok" for r in results)
    assert elapsed_ms < 200, (
        f"Python-callable chat() serialised: {elapsed_ms:.1f}ms for 5 concurrent calls "
        f"(expected ~50ms; >200ms means the GIL leaked across the Rust→Python→Rust hop)"
    )
