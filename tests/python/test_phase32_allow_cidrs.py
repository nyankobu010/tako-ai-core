"""Phase 32.C — Python facade smoke for the new
``url_prefetch_allow_cidrs`` kwarg on ``tako.providers.Bedrock``
and ``tako.providers.Ollama``.

Phase 30 + 31 shipped host-string allowlists (exact + wildcard
suffix). Phase 32.A/B added CIDR subnet allowlists on the Rust
side; Phase 32.C threads the new kwarg through the Python
facade.

CIDR strings (`"10.0.5.0/24"`, `"2001:db8::/32"`) match any
resolved IP that falls inside the network — useful for subnets
without a shared DNS suffix or for raw IP-literal URLs. CIDR
parse failures surface from the constructor as an exception.

The Bedrock and Ollama constructors call live SDKs / daemons; on
hosts without AWS credentials or an Ollama daemon they can fail
at construction. This file therefore validates the Python-facing
*signature* (kwarg accepted, default ``None``, type stubs in
sync). The Rust unit tests in
``crates/tako-providers/{bedrock,ollama}/src/url_prefetch.rs``
remain the source of truth for behaviour (16 new Phase 32 tests
across 32.A + 32.B).
"""

from __future__ import annotations

import inspect

from tako import providers


def test_bedrock_constructor_signature_includes_allow_cidrs_kwarg() -> None:
    """The new Phase 32.C kwarg is present in the
    ``tako.providers.Bedrock.__init__`` signature."""
    sig = inspect.signature(providers.Bedrock.__init__)
    assert "url_prefetch_allow_cidrs" in sig.parameters


def test_bedrock_allow_cidrs_default_is_none() -> None:
    """``None`` means no CIDR allowlist (default)."""
    sig = inspect.signature(providers.Bedrock.__init__)
    assert sig.parameters["url_prefetch_allow_cidrs"].default is None


def test_ollama_constructor_signature_includes_allow_cidrs_kwarg() -> None:
    sig = inspect.signature(providers.Ollama.__init__)
    assert "url_prefetch_allow_cidrs" in sig.parameters


def test_ollama_allow_cidrs_default_is_none() -> None:
    sig = inspect.signature(providers.Ollama.__init__)
    assert sig.parameters["url_prefetch_allow_cidrs"].default is None


def test_bedrock_docstring_documents_allow_cidrs() -> None:
    """The Phase 32 CIDR allowlist is documented on the Bedrock
    class docstring with a recognisable CIDR example."""
    docstring = providers.Bedrock.__doc__ or ""
    assert "url_prefetch_allow_cidrs" in docstring
    # Either of the canonical CIDR examples should appear.
    assert "/24" in docstring or "/32" in docstring or "CIDR" in docstring


def test_ollama_docstring_documents_allow_cidrs() -> None:
    docstring = providers.Ollama.__doc__ or ""
    assert "url_prefetch_allow_cidrs" in docstring
    assert "/24" in docstring or "/32" in docstring or "CIDR" in docstring


def test_bedrock_docstring_mentions_three_allowlist_forms() -> None:
    """After Phase 32, the operator allowlist surface covers
    three forms: exact host, wildcard host, CIDR subnet. The
    Bedrock docstring should mention all three so users discover
    them."""
    docstring = providers.Bedrock.__doc__ or ""
    assert "url_prefetch_allow_hosts" in docstring
    assert "url_prefetch_allow_cidrs" in docstring
    # Wildcard syntax marker.
    assert "*." in docstring
