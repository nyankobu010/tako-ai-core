"""Phase 29.C — Python facade smoke for the new
``tako.providers.Ollama`` class.

Phase 28.C threaded the URL pre-fetch kwargs through
``tako.providers.Bedrock`` only — Ollama had no Python binding
in tako-py. Phase 29.C closes that asymmetry by mirroring the
``PyBedrock`` cadence under ``PyOllama``.

The Ollama constructor calls a live HTTP daemon; on hosts without
an Ollama daemon it can fail at construction. This file therefore
validates the Python-facing *signature* (class exists, kwargs
accepted, defaults sensible, type stubs in sync) rather than
constructing live providers. The Rust unit tests in
``crates/tako-providers/ollama/src/url_prefetch.rs`` and
``crates/tako-providers/ollama/src/client.rs`` remain the source
of truth for behaviour.
"""

from __future__ import annotations

import inspect

from tako import providers
from tako import _native


def test_ollama_class_exists() -> None:
    """Phase 29.C — the Ollama facade class is reachable from
    ``tako.providers``."""
    assert hasattr(providers, "Ollama")
    assert isinstance(providers.Ollama, type)


def test_native_ollama_class_exists() -> None:
    """The underlying PyO3 class is registered in the
    ``tako._native`` extension."""
    assert hasattr(_native, "Ollama")


def test_ollama_constructor_signature() -> None:
    """The Phase 29.C constructor exposes the expected kwargs:
    ``model`` (positional), ``base_url``, ``timeout_secs``, plus
    the five url_prefetch_* knobs (mirroring Phase 28.C/29.C
    Bedrock surface minus AWS-specific ``region`` /
    ``endpoint_url`` / ``profile_name``)."""
    sig = inspect.signature(providers.Ollama.__init__)
    params = sig.parameters
    expected = {
        "model",
        "base_url",
        "timeout_secs",
        "url_prefetch",
        "url_prefetch_allow_http",
        "url_prefetch_allow_private_ips",
        "url_prefetch_timeout_secs",
        "url_prefetch_max_bytes",
    }
    for name in expected:
        assert name in params, f"missing kwarg: {name}"


def test_ollama_url_prefetch_defaults() -> None:
    """All four boolean url_prefetch flags default to False (opt-in
    pattern); the two override Optionals default to None (use the
    Rust-side defaults: 10s / 10 MiB)."""
    sig = inspect.signature(providers.Ollama.__init__)
    params = sig.parameters
    assert params["url_prefetch"].default is False
    assert params["url_prefetch_allow_http"].default is False
    assert params["url_prefetch_allow_private_ips"].default is False
    assert params["url_prefetch_timeout_secs"].default is None
    assert params["url_prefetch_max_bytes"].default is None


def test_ollama_base_url_default_is_none() -> None:
    """``base_url=None`` lets the Rust side pick the default
    ``http://localhost:11434``."""
    sig = inspect.signature(providers.Ollama.__init__)
    assert sig.parameters["base_url"].default is None


def test_ollama_inherits_provider_base() -> None:
    """``Ollama`` inherits from ``_ProviderBase`` so the ``id``
    accessor pattern works the same as every other provider."""
    # _ProviderBase is private; assert via hasattr on the property.
    assert hasattr(providers.Ollama, "id")


def test_ollama_docstring_documents_url_prefetch() -> None:
    """The Phase 29 SSRF mitigation surface is documented on the
    class docstring."""
    docstring = providers.Ollama.__doc__ or ""
    assert "url_prefetch" in docstring
    # Either of the canonical security-feature mentions is enough.
    assert "https" in docstring.lower() or "SSRF" in docstring
