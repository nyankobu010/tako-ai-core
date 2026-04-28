"""Tracing setup."""

from __future__ import annotations

from tako import _native


def init(*, filter: str | None = None, json: bool = False) -> None:
    """Initialise process-wide tracing. Idempotent."""
    _native.init_tracing_py(filter, json)


class Otlp:
    """Configure an OTLP exporter (Phase 2). Today this stores the endpoint
    so callers can write the eventual API now; the OTLP exporter wires up
    in Phase 2."""

    def __init__(self, endpoint: str) -> None:
        self.endpoint = endpoint

    def __repr__(self) -> str:
        return f"Otlp(endpoint={self.endpoint!r})"
