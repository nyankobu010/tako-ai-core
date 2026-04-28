"""Provider adapters: thin wrappers around the native classes.

Each adapter exposes a stable, kwargs-friendly Python constructor and
forwards to the underlying Rust builder via ``tako._native``.
"""

from __future__ import annotations

from typing import Any

from tako import _native


class _ProviderBase:
    """Common attributes; a subclass of this is what callers receive back
    from each provider constructor. The ``_handle`` attribute carries the
    native object that ``Orchestrator`` accepts."""

    _handle: Any

    @property
    def id(self) -> str:
        return str(self._handle.id())


class OpenAI(_ProviderBase):
    """OpenAI chat.completions provider."""

    def __init__(
        self,
        model: str,
        api_key: str,
        *,
        base_url: str | None = None,
        timeout_secs: int | None = None,
        organization: str | None = None,
    ) -> None:
        self._handle = _native.OpenAI(
            model,
            api_key,
            base_url=base_url,
            timeout_secs=timeout_secs,
            organization=organization,
        )


class Anthropic(_ProviderBase):
    """Anthropic Messages API provider."""

    def __init__(
        self,
        model: str,
        api_key: str,
        *,
        base_url: str | None = None,
        timeout_secs: int | None = None,
        default_max_tokens: int | None = None,
    ) -> None:
        self._handle = _native.Anthropic(
            model,
            api_key,
            base_url=base_url,
            timeout_secs=timeout_secs,
            default_max_tokens=default_max_tokens,
        )


class Fake(_ProviderBase):
    """In-process fake provider for tests. Returns canned text and tracks call count."""

    def __init__(
        self,
        canned_text: str = "ok",
        *,
        id: str = "fake:test",
        delay_ms: int = 0,
    ) -> None:
        self._handle = _native.FakeProvider(canned_text, id, delay_ms)

    @property
    def call_count(self) -> int:
        return int(self._handle.call_count())
