"""Smoke tests for the Vertex AI (Gemini) provider Python binding.

Wire-format correctness is covered by the Rust crate's wiremock tests; this
file just verifies the Python facade constructs the native handle and
surfaces the right id, plus that the orchestrator accepts Vertex providers.
"""

from __future__ import annotations

import os

import pytest
import tako


def test_vertex_id() -> None:
    p = tako.providers.Vertex(
        project_id="my-proj",
        model="gemini-2.0-pro",
        access_token="ya29.test",
    )
    assert p.id == "vertex:gemini-2.0-pro"


def test_vertex_with_custom_location_and_endpoint() -> None:
    p = tako.providers.Vertex(
        project_id="my-proj",
        model="gemini-2.0-flash",
        access_token="ya29.test",
        location="europe-west4",
        endpoint_url="https://example.test",
        timeout_secs=30,
    )
    assert p.id == "vertex:gemini-2.0-flash"


def test_vertex_env_indirect_missing_token_raises() -> None:
    with pytest.raises(ValueError):
        tako.providers.Vertex(
            project_id="my-proj",
            model="gemini-2.0-pro",
            access_token="$ENV:DEFINITELY_NOT_SET_VERTEX_TOKEN_XYZ",
        )


def test_vertex_env_indirect_resolves() -> None:
    os.environ["TEST_VERTEX_TOKEN"] = "ya29.from-env"
    try:
        p = tako.providers.Vertex(
            project_id="my-proj",
            model="gemini-2.0-pro",
            access_token="$ENV:TEST_VERTEX_TOKEN",
        )
        assert p.id == "vertex:gemini-2.0-pro"
    finally:
        del os.environ["TEST_VERTEX_TOKEN"]


def test_vertex_works_in_orchestrator() -> None:
    p = tako.providers.Vertex(
        project_id="my-proj",
        model="gemini-2.0-pro",
        access_token="ya29.test",
    )
    agent = tako.SingleAgent(provider=p)
    assert agent is not None
