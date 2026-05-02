"""End-to-end smoke tests against the in-process FakeProvider."""

from __future__ import annotations

import tako


async def test_import_and_version() -> None:
    assert tako.__version__ == "0.44.0"
    # Native module exposes its own version too.
    from tako import _native

    assert _native.__version__ == "0.44.0"


async def test_single_agent_run_with_fake(fake_provider: tako.providers.Fake) -> None:
    agent = tako.SingleAgent(provider=fake_provider)
    result = await agent.run("anything")
    assert result.text == "hello from fake"
    assert fake_provider.call_count == 1


def test_single_agent_run_sync(fake_provider: tako.providers.Fake) -> None:
    agent = tako.SingleAgent(provider=fake_provider)
    result = agent.run_sync("anything")
    assert result.text == "hello from fake"
    assert fake_provider.call_count == 1


def test_provider_id() -> None:
    fake = tako.providers.Fake(canned_text="x", id="custom:fake")
    assert fake.id == "custom:fake"


def test_orchestrator_rejects_non_provider() -> None:
    import pytest

    with pytest.raises(TypeError):
        tako.SingleAgent(provider="not a provider")  # type: ignore[arg-type]


def test_budget_constructs() -> None:
    b = tako.Budget(max_usd_per_request=5.0, max_usd_per_day=500.0)
    assert "Budget" in repr(b)


def test_pydantic_models_round_trip() -> None:
    msg = tako.Message.user("hello")
    assert msg.role == tako.Role.USER
    serialised = msg.model_dump()
    restored = tako.Message.model_validate(serialised)
    assert restored == msg
