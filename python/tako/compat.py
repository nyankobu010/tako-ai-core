"""OpenAI-compatible HTTP server."""

from __future__ import annotations

from typing import Any

from tako import _native


def serve_openai(
    orch: Any,
    *,
    host: str = "127.0.0.1",
    port: int = 8080,
    tokens: dict[str, tuple[str, str]] | None = None,
    models: list[str] | None = None,
) -> str:
    """Boot the OpenAI-compatible HTTP server.

    Returns the bound URL (e.g. ``"http://127.0.0.1:8080"``). The server
    runs in a background Tokio task; call :func:`shutdown_openai` to stop
    it. ``orch`` must be a ``tako.SingleAgent`` or ``tako.Conductor``.

    Auth: every request must carry ``Authorization: Bearer <token>``.
    ``tokens`` maps each token to ``(tenant_id, user_id)``. If you don't
    pass anything, a single ``"dev-token"`` mapping to anonymous is
    installed for easy local testing — production deployments must pass
    their own map.
    """
    if not hasattr(orch, "_inner"):
        raise TypeError("orch must be a tako.SingleAgent or tako.Conductor instance")
    return _native.serve_openai_py(
        orch._inner,
        host=host,
        port=port,
        tokens=tokens,
        models=models,
    )


def shutdown_openai() -> None:
    """Stop the running compat server. Idempotent."""
    _native.shutdown_compat_py()


__all__ = ["serve_openai", "shutdown_openai"]
