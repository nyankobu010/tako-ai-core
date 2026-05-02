"""Phase 29.C — Python facade smoke for the new
``url_prefetch_allow_private_ips`` kwarg on ``tako.providers.Bedrock``
and ``tako.providers.Ollama``.

Phase 28 shipped the four URL pre-fetch kwargs (``url_prefetch``,
``url_prefetch_allow_http``, ``url_prefetch_timeout_secs``,
``url_prefetch_max_bytes``) on Bedrock only. Phase 29.A/B added the
fifth kwarg (``url_prefetch_allow_private_ips``) on the Rust side
covering BOTH providers; Phase 29.C threads it through the Python
facade.

The Bedrock and Ollama constructors call live SDKs / daemons; on
hosts without AWS credentials or an Ollama daemon they can fail at
construction. This file therefore validates the Python-facing
*signature* (kwarg accepted, default sensible, type stubs in sync)
rather than constructing live providers. The Rust unit tests in
``crates/tako-providers/{bedrock,ollama}/src/url_prefetch.rs``
remain the source of truth for behaviour (28 new Phase 29 tests
across 29.A + 29.B).
"""

from __future__ import annotations

import inspect

from tako import providers


def test_bedrock_constructor_signature_includes_allow_private_ips_kwarg() -> None:
    """The new Phase 29.C kwarg is present in the
    ``tako.providers.Bedrock.__init__`` signature."""
    sig = inspect.signature(providers.Bedrock.__init__)
    assert "url_prefetch_allow_private_ips" in sig.parameters


def test_bedrock_allow_private_ips_default_is_false() -> None:
    """Phase 29.A semantics: default-deny stance for SSRF. The
    blocklist is ON by default; this kwarg is the opt-out."""
    sig = inspect.signature(providers.Bedrock.__init__)
    assert sig.parameters["url_prefetch_allow_private_ips"].default is False


def test_ollama_constructor_signature_includes_allow_private_ips_kwarg() -> None:
    """The new Phase 29.C kwarg is present in the
    ``tako.providers.Ollama.__init__`` signature."""
    sig = inspect.signature(providers.Ollama.__init__)
    assert "url_prefetch_allow_private_ips" in sig.parameters


def test_ollama_allow_private_ips_default_is_false() -> None:
    """Phase 29.B semantics mirror 29.A: default-deny SSRF stance."""
    sig = inspect.signature(providers.Ollama.__init__)
    assert sig.parameters["url_prefetch_allow_private_ips"].default is False


def test_bedrock_docstring_documents_phase29_blocklist() -> None:
    """The Phase 29 SSRF mitigation surface is documented on the
    ``tako.providers.Bedrock`` class docstring."""
    docstring = providers.Bedrock.__doc__ or ""
    assert "url_prefetch_allow_private_ips" in docstring
    # At least one of the canonical SSRF target IP families is named.
    assert any(token in docstring for token in ("loopback", "RFC 1918", "link-local", "169.254"))


def test_ollama_docstring_documents_phase29_blocklist() -> None:
    """The Phase 29 SSRF mitigation surface is documented on the
    ``tako.providers.Ollama`` class docstring."""
    docstring = providers.Ollama.__doc__ or ""
    assert "url_prefetch_allow_private_ips" in docstring
    assert any(token in docstring for token in ("loopback", "RFC 1918", "link-local", "private"))
