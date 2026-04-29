"""Conductor end-to-end test from Python.

Uses PythonProvider for both the coordinator and workers so the test
runs without API keys. The coordinator emits scripted dispatch JSON;
workers return canned text.
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import Any

import pytest
import tako


def _coord_factory(scripts: list[str]) -> Any:
    """Yields each script in turn from the coordinator's chat callable."""
    it: Iterator[str] = iter(scripts)

    async def chat(_request: dict[str, Any]) -> str:
        try:
            return next(it)
        except StopIteration as e:
            raise AssertionError("coordinator called more times than scripted") from e

    return chat


async def _worker_chat(_request: dict[str, Any]) -> str:
    return "worker-ok"


async def test_conductor_dispatches_and_halts() -> None:
    coord = tako.providers.PythonProvider(
        "py:coord",
        chat=_coord_factory(
            [
                '{"thought":"go","dispatch":[{"worker":"code","task":"a"},{"worker":"math","task":"b"}],"halt":false}',
                '{"thought":"done","dispatch":[],"halt":true,"final_answer":"all-done"}',
            ]
        ),
    )
    code = tako.providers.PythonProvider("py:code", chat=_worker_chat)
    math = tako.providers.PythonProvider("py:math", chat=_worker_chat)

    cond = tako.Conductor(
        coordinator=coord,
        workers={"code": code, "math": math},
        max_steps=4,
    )
    result = await cond.run("plan + verify")
    assert result.text == "all-done"


async def test_conductor_rejects_non_provider() -> None:
    coord = tako.providers.PythonProvider("py:coord", chat=_worker_chat)
    with pytest.raises(TypeError):
        tako.Conductor(coordinator=coord, workers={"x": "not a provider"})  # type: ignore[arg-type]
