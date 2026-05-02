"""Phase 46.B — orchestrator result exposes ``usage`` + ``steps``.

Phase 1 shipped ``_Result(text)`` as a placeholder — orchestrators
returned only the assistant text and discarded ``usage`` / ``steps``
that the Rust ``OrchOutput`` struct already carried. Phase 46.B
plumbs both fields through:

- ``result.text`` — unchanged, still stable.
- ``result.usage.input_tokens`` / ``output_tokens`` / ``total`` —
  Pydantic ``Usage`` model.
- ``result.steps`` — number of provider calls (``1`` for a
  single-shot ``FakeProvider`` answer).

These tests use ``FakeProvider`` so no API keys / network is
needed. They run on every CI configuration.
"""

from __future__ import annotations

import tako


async def test_single_agent_result_carries_usage_and_steps() -> None:
    fake = tako.providers.Fake(canned_text="hello from fake")
    agent = tako.SingleAgent(provider=fake)
    result = await agent.run("anything")

    # Stable Phase-1 field.
    assert result.text == "hello from fake"

    # Phase 46.B additive fields.
    assert result.steps == 1, f"FakeProvider single-shot run should have 1 step; got {result.steps}"
    assert result.usage.input_tokens >= 0
    assert result.usage.output_tokens >= 0
    assert result.usage.total == (result.usage.input_tokens + result.usage.output_tokens)


def test_single_agent_run_sync_result_carries_usage_and_steps() -> None:
    fake = tako.providers.Fake(canned_text="sync answer")
    agent = tako.SingleAgent(provider=fake)
    result = agent.run_sync("anything")

    assert result.text == "sync answer"
    assert result.steps == 1
    assert result.usage.total == (result.usage.input_tokens + result.usage.output_tokens)


async def test_result_repr_includes_usage_and_steps() -> None:
    """The ``repr`` should expose the new fields so debugging /
    logging shows them without manual unpacking."""
    fake = tako.providers.Fake(canned_text="x")
    agent = tako.SingleAgent(provider=fake)
    result = await agent.run("y")
    text_repr = repr(result)
    assert "OrchOutput" in text_repr
    assert "input_tokens=" in text_repr
    assert "output_tokens=" in text_repr
    assert "steps=" in text_repr


async def test_result_text_field_remains_str() -> None:
    """Existing callers of ``result.text`` must not see any type
    change. (Phase 1's stability promise.)"""
    fake = tako.providers.Fake(canned_text="just a string")
    agent = tako.SingleAgent(provider=fake)
    result = await agent.run("anything")
    assert isinstance(result.text, str)
    assert result.text == "just a string"
