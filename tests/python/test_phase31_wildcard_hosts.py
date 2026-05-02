"""Phase 31.C — Python facade smoke for wildcard host
patterns in the URL pre-fetch allowlist.

Phase 30 shipped an exact-string allowlist via
``url_prefetch_allow_hosts: list[str] | None`` on both
``tako.providers.Bedrock`` and ``tako.providers.Ollama``. Phase
31 extends the matching semantic on the Rust side: entries
starting with ``*.`` are recognised as wildcard suffix patterns
that match any hostname ending in ``.X``, including multi-level
subdomains.

The Python kwarg shape doesn't change in Phase 31 (still
``list[str] | None``) — the new behaviour is entirely on the
Rust side. This file pins:

1. The kwarg type still accepts ``list[str] | None`` (regression).
2. Both providers' docstrings document the wildcard semantic so
   end users discover it.

Behaviour pinned in the Rust unit tests at
``crates/tako-providers/{bedrock,ollama}/src/url_prefetch.rs``
(8 new Phase 31 tests per crate covering exact-match regression,
single-level and multi-level subdomain match, bare-domain
non-match, attacker-domain non-match, and exact + wildcard
coexistence).
"""

from __future__ import annotations

import inspect

from tako import providers


def test_bedrock_allow_hosts_kwarg_type_unchanged() -> None:
    """The Phase 30 kwarg shape (``list[str] | None``) is
    preserved through the Phase 31 wildcard extension."""
    sig = inspect.signature(providers.Bedrock.__init__)
    param = sig.parameters["url_prefetch_allow_hosts"]
    assert param.default is None


def test_ollama_allow_hosts_kwarg_type_unchanged() -> None:
    sig = inspect.signature(providers.Ollama.__init__)
    param = sig.parameters["url_prefetch_allow_hosts"]
    assert param.default is None


def test_bedrock_docstring_documents_wildcard_pattern() -> None:
    """The Phase 31 wildcard semantic is documented on the
    Bedrock class docstring so users discover it."""
    docstring = providers.Bedrock.__doc__ or ""
    assert "*.internal.corp" in docstring or "*." in docstring
    # The multi-level matching nuance must be reflected somewhere
    # readable.
    assert "subdomain" in docstring.lower() or "multi-level" in docstring.lower()


def test_ollama_docstring_documents_wildcard_pattern() -> None:
    docstring = providers.Ollama.__doc__ or ""
    assert "*.internal.corp" in docstring or "*." in docstring
    assert "subdomain" in docstring.lower() or "multi-level" in docstring.lower()


def test_bedrock_docstring_includes_apex_caveat() -> None:
    """The bare-domain non-match caveat (`*.X` does NOT match `X`)
    is the most surprising part of the wildcard semantics — users
    must be able to discover it from the docstring."""
    docstring = providers.Bedrock.__doc__ or ""
    assert "apex" in docstring.lower() or "bare" in docstring.lower()


def test_ollama_docstring_includes_apex_caveat() -> None:
    docstring = providers.Ollama.__doc__ or ""
    assert "apex" in docstring.lower() or "bare" in docstring.lower()
