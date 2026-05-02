"""Phase 2 example: Conductor orchestrator with two specialised workers.

Falls back to a pure-Python coordinator + workers when no API keys are
set, so the example runs end-to-end during CI smoke tests.

Run:
    pip install tako
    export ANTHROPIC_API_KEY=...   # optional
    export OPENAI_API_KEY=...      # optional
    python examples/02_conductor.py
"""

from __future__ import annotations

import asyncio
import os
from typing import Any

import tako


async def _fake_coord(_request: dict[str, Any]) -> str:
    """Scripted coordinator: dispatch once, then halt with the workers' summary."""
    user_text = _request["messages"][-1]["content"][0]["text"]
    if "Worker results" in user_text:
        return (
            '{"thought":"workers replied","dispatch":[],"halt":true,'
            '"final_answer":"Done — see worker results above."}'
        )
    return (
        '{"thought":"plan","dispatch":['
        '{"worker":"code","task":"Outline a fibonacci impl."},'
        '{"worker":"math","task":"State the recurrence."}'
        '],"halt":false}'
    )


async def _fake_worker(name: str) -> Any:
    async def chat(_request: dict[str, Any]) -> str:
        return f"({name}) result"

    return chat


async def main() -> None:
    anthropic_key = os.getenv("ANTHROPIC_API_KEY")
    openai_key = os.getenv("OPENAI_API_KEY")
    if anthropic_key and openai_key:
        coordinator = tako.providers.Anthropic(model="claude-opus-4-7", api_key=anthropic_key)
        code = tako.providers.OpenAI(model="gpt-5", api_key=openai_key)
        math = tako.providers.Anthropic(model="claude-opus-4-7", api_key=anthropic_key)
    else:
        coordinator = tako.providers.PythonProvider("py:coord", chat=_fake_coord)
        code = tako.providers.PythonProvider("py:code", chat=await _fake_worker("code"))
        math = tako.providers.PythonProvider("py:math", chat=await _fake_worker("math"))

    cond = tako.Conductor(
        coordinator=coordinator,
        workers={"code": code, "math": math},
        max_fanout=2,
        max_steps=4,
    )
    result = await cond.run("Plan + verify a fibonacci function.")
    print(result.text)


if __name__ == "__main__":
    asyncio.run(main())
