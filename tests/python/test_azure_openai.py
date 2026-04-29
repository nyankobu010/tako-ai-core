"""Smoke tests for the Azure OpenAI provider Python binding.

Wire-format correctness (URL shape, api-key header, SSE) is covered by the
Rust crate's wiremock tests; this file just verifies the Python facade
constructs the native handle and surfaces the right id.
"""

from __future__ import annotations

import pytest

import tako


def test_azure_openai_id() -> None:
    p = tako.providers.AzureOpenAI(
        endpoint="https://my-resource.openai.azure.com",
        deployment="gpt-4o-prod",
        api_key="test",
    )
    assert p.id == "azure-openai:gpt-4o-prod"


def test_azure_openai_with_custom_api_version() -> None:
    p = tako.providers.AzureOpenAI(
        endpoint="https://my-resource.openai.azure.com",
        deployment="d1",
        api_key="test",
        api_version="2025-01-01-preview",
        timeout_secs=30,
    )
    assert p.id == "azure-openai:d1"


def test_azure_openai_env_indirect_missing_key_raises() -> None:
    """`$ENV:VAR` resolution fails at build time when the var is unset."""
    with pytest.raises(ValueError):
        tako.providers.AzureOpenAI(
            endpoint="https://x.openai.azure.com",
            deployment="d1",
            api_key="$ENV:DEFINITELY_NOT_SET_VAR_XYZ",
        )


def test_azure_openai_accepts_env_indirect_key() -> None:
    import os

    os.environ["TEST_AZURE_OPENAI_API_KEY"] = "from-env"
    try:
        p = tako.providers.AzureOpenAI(
            endpoint="https://x.openai.azure.com",
            deployment="d1",
            api_key="$ENV:TEST_AZURE_OPENAI_API_KEY",
        )
        assert p.id == "azure-openai:d1"
    finally:
        del os.environ["TEST_AZURE_OPENAI_API_KEY"]


def test_azure_openai_works_in_orchestrator() -> None:
    """Orchestrator construction accepts AzureOpenAI as a provider type."""
    p = tako.providers.AzureOpenAI(
        endpoint="https://x.openai.azure.com",
        deployment="d1",
        api_key="test",
    )
    agent = tako.SingleAgent(provider=p)
    assert agent is not None
