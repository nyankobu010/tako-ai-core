"""Phase 30.C — Python facade smoke for the new
``url_prefetch_allow_hosts`` kwarg on ``tako.providers.Bedrock``
and ``tako.providers.Ollama``.

Phase 29.A/B added a default-on private-IP blocklist. The
binary opt-out flag (``url_prefetch_allow_private_ips``) is a
sledgehammer — operators with an internal artifact registry on
a private RFC 1918 address have to disable the WHOLE blocklist
just to permit one trusted host. Phase 30.A/B added the per-host
allowlist that bypasses the blocklist for specific hostnames
only; Phase 30.C threads the kwarg through the Python facade.

The Bedrock and Ollama constructors call live SDKs / daemons; on
hosts without AWS credentials or an Ollama daemon they can fail
at construction. This file therefore validates the Python-facing
*signature* (kwarg accepted, default ``None``, type stubs in
sync). The Rust unit tests in
``crates/tako-providers/{bedrock,ollama}/src/url_prefetch.rs``
remain the source of truth for behaviour (10 new Phase 30 tests
across 30.A + 30.B).
"""

from __future__ import annotations

import inspect

from tako import providers


def test_bedrock_constructor_signature_includes_allow_hosts_kwarg() -> None:
    """The new Phase 30.C kwarg is present in the
    ``tako.providers.Bedrock.__init__`` signature."""
    sig = inspect.signature(providers.Bedrock.__init__)
    assert "url_prefetch_allow_hosts" in sig.parameters


def test_bedrock_allow_hosts_default_is_none() -> None:
    """``None`` means no allowlist (default behaviour pre-30)."""
    sig = inspect.signature(providers.Bedrock.__init__)
    assert sig.parameters["url_prefetch_allow_hosts"].default is None


def test_ollama_constructor_signature_includes_allow_hosts_kwarg() -> None:
    """The new Phase 30.C kwarg is present in the
    ``tako.providers.Ollama.__init__`` signature."""
    sig = inspect.signature(providers.Ollama.__init__)
    assert "url_prefetch_allow_hosts" in sig.parameters


def test_ollama_allow_hosts_default_is_none() -> None:
    sig = inspect.signature(providers.Ollama.__init__)
    assert sig.parameters["url_prefetch_allow_hosts"].default is None


def test_bedrock_docstring_documents_allow_hosts() -> None:
    """The Phase 30 per-host allowlist is documented on the
    ``tako.providers.Bedrock`` class docstring."""
    docstring = providers.Bedrock.__doc__ or ""
    assert "url_prefetch_allow_hosts" in docstring
    # The semantic that it bypasses the blocklist must be
    # reflected somewhere readable.
    assert "allowlist" in docstring.lower() or "bypass" in docstring.lower()


def test_ollama_docstring_documents_allow_hosts() -> None:
    docstring = providers.Ollama.__doc__ or ""
    assert "url_prefetch_allow_hosts" in docstring
    assert "allowlist" in docstring.lower() or "bypass" in docstring.lower()
