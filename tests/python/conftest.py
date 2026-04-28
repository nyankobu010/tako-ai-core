"""Shared pytest fixtures."""

from __future__ import annotations

import pytest

import tako


@pytest.fixture
def fake_provider() -> tako.providers.Fake:
    return tako.providers.Fake(canned_text="hello from fake")


@pytest.fixture
def fake_with_delay() -> tako.providers.Fake:
    """50ms delay per call so concurrency tests can detect serialisation."""
    return tako.providers.Fake(canned_text="ok", delay_ms=50)
