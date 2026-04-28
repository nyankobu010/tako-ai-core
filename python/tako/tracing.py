"""Tracing setup."""

from __future__ import annotations

from tako import _native


def init(*, filter: str | None = None, json: bool = False) -> None:
    """Initialise process-wide tracing without an OTLP exporter (stderr only).
    Idempotent."""
    _native.init_tracing_py(filter, json)


def init_otlp(
    endpoint: str,
    *,
    filter: str | None = None,
    json: bool = False,
) -> None:
    """Initialise tracing **with** an OTLP gRPC exporter.

    Spans land in the collector at ``endpoint`` (e.g. ``http://localhost:4317``).
    A process-global guard keeps the exporter alive for the interpreter's
    lifetime; pending spans flush at exit. Calling twice without an
    intervening :func:`shutdown_otlp` is rejected.
    """
    _native.init_otlp_tracing_py(endpoint, filter, json)


def shutdown_otlp() -> None:
    """Drop the OTLP exporter, flushing pending spans. Idempotent."""
    _native.shutdown_otlp_py()


class Otlp:
    """Lazy config object — most users will call :func:`init_otlp` directly.
    This class exists so user code can write ``tako.tracing.Otlp(endpoint=...)``
    in a Client constructor and have tako pick up the endpoint later."""

    def __init__(self, endpoint: str) -> None:
        self.endpoint = endpoint

    def install(self, *, filter: str | None = None, json: bool = False) -> None:
        """Apply the configuration via :func:`init_otlp`."""
        init_otlp(self.endpoint, filter=filter, json=json)

    def __repr__(self) -> str:
        return f"Otlp(endpoint={self.endpoint!r})"
