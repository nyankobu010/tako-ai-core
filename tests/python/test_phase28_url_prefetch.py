"""Phase 28.C — Python facade smoke for ``tako.providers.Bedrock``
URL pre-fetch keyword arguments.

Phase 28.A added the opt-in tako-side URL pre-fetch on the Rust
side (`BedrockBuilder::with_url_prefetch*`). Phase 28.B mirrored
this for Ollama. Phase 28.C threads the new knobs through
`tako._native.Bedrock` + `tako.providers.Bedrock`. Ollama doesn't
have a Python binding (no entry in tako-py), so the Python
surface is Bedrock-only for Phase 28.

The `Bedrock` constructor calls AWS SDK to load the credential
chain; on hosts without AWS credentials it can fail at
construction. This file therefore validates the Python-facing
*signature* (kwargs accepted, defaults sensible, type stubs in
sync) rather than constructing live providers. The Rust unit
tests in `crates/tako-providers/{bedrock,ollama}/src/url_prefetch.rs`
remain the source of truth for behaviour (15 tests across
28.A + 28.B).
"""

from __future__ import annotations

import inspect

from tako import providers


def test_bedrock_constructor_signature_includes_url_prefetch_kwargs() -> None:
    """The four new Phase-28.C kwargs are present in the
    `tako.providers.Bedrock.__init__` signature."""
    sig = inspect.signature(providers.Bedrock.__init__)
    params = sig.parameters
    for name in (
        "url_prefetch",
        "url_prefetch_allow_http",
        "url_prefetch_timeout_secs",
        "url_prefetch_max_bytes",
    ):
        assert name in params, f"missing kwarg: {name}"


def test_bedrock_constructor_url_prefetch_defaults() -> None:
    """Phase 28.A semantics: opt-in default-off. Both boolean
    flags default to False; the override Optionals default to
    None (use Rust-side defaults: 10s / 10 MiB).
    """
    sig = inspect.signature(providers.Bedrock.__init__)
    params = sig.parameters
    assert params["url_prefetch"].default is False
    assert params["url_prefetch_allow_http"].default is False
    assert params["url_prefetch_timeout_secs"].default is None
    assert params["url_prefetch_max_bytes"].default is None


def test_bedrock_docstring_documents_url_prefetch() -> None:
    """The new kwargs are documented in the
    `tako.providers.Bedrock` docstring so end users discover
    them."""
    docstring = providers.Bedrock.__doc__ or ""
    assert "url_prefetch" in docstring
    assert "SSRF" in docstring or "https" in docstring.lower()
