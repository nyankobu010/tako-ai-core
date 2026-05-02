"""Phase 12.B — smoke tests for ``tako.providers.HttpGeneric``.

The Rust crate is fully covered by ``crates/tako-providers/http-generic``'s
own ``wiremock`` suite; these tests verify only that the Python facade
marshals constructor arguments correctly into the underlying Rust config
and surfaces the streaming-capability bit.
"""

from __future__ import annotations

import pytest
import tako


def test_http_generic_construction_minimal() -> None:
    p = tako.providers.HttpGeneric(
        id="custom-1",
        model="my-model",
        url="http://example.invalid/v1/chat",
        body_template={"model": "{{ model }}"},
        response_text_pointer="/text",
    )
    assert p.id == "custom-1"
    # No stream_config supplied → streaming capability stays off.
    assert p.supports_streaming is False


def test_http_generic_stream_config_openai_sse_flips_streaming() -> None:
    p = tako.providers.HttpGeneric(
        id="streaming",
        model="m",
        url="http://example.invalid/v1/chat",
        body_template={"model": "{{ model }}"},
        response_text_pointer="/text",
        stream_config={"kind": "openai_sse"},
    )
    assert p.supports_streaming is True


def test_http_generic_stream_config_ndjson_flips_streaming() -> None:
    p = tako.providers.HttpGeneric(
        id="ndjson",
        model="m",
        url="http://example.invalid/v1/chat",
        body_template={"model": "{{ model }}"},
        response_text_pointer="/text",
        stream_config={
            "kind": "ndjson",
            "content_pointer": "/delta",
            "finish_reason_pointer": "/done",
        },
    )
    assert p.supports_streaming is True


def test_http_generic_validation_errors_surface() -> None:
    # Empty `id` is rejected by the Rust-side validator.
    with pytest.raises(ValueError, match="id, model, url are required"):
        tako.providers.HttpGeneric(
            id="",
            model="m",
            url="http://example.invalid/v1/chat",
            body_template={},
            response_text_pointer="/text",
        )


def test_http_generic_unknown_stream_config_kind_raises() -> None:
    with pytest.raises(ValueError, match="invalid stream_config"):
        tako.providers.HttpGeneric(
            id="x",
            model="m",
            url="http://example.invalid/v1/chat",
            body_template={},
            response_text_pointer="/text",
            stream_config={"kind": "not-a-real-shape"},
        )


def test_http_generic_plugs_into_single_agent() -> None:
    # The orchestrator's provider-extraction chain must accept HttpGeneric
    # alongside OpenAI / Anthropic / Bedrock / etc. Construction should
    # succeed; we don't run the agent (the URL is unreachable).
    provider = tako.providers.HttpGeneric(
        id="x",
        model="m",
        url="http://example.invalid/v1/chat",
        body_template={"model": "{{ model }}"},
        response_text_pointer="/text",
    )
    agent = tako.SingleAgent(provider=provider)
    assert agent is not None
