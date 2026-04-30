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
    auth: Any = None,
    models: list[str] | None = None,
) -> str:
    """Boot the OpenAI-compatible HTTP server.

    Returns the bound URL (e.g. ``"http://127.0.0.1:8080"``). The server
    runs in a background Tokio task; call :func:`shutdown_openai` to stop
    it. ``orch`` must be a ``tako.SingleAgent`` or ``tako.Conductor``.

    Auth: every request must carry ``Authorization: Bearer <token>``.

    Two auth modes:

    1. **Static map** (dev / CI): ``tokens={"my-token": ("acme", "alice")}``.
       Each token maps to ``(tenant_id, user_id)``. If neither ``tokens``
       nor ``auth`` is set, a single ``"dev-token"`` mapping to anonymous
       is installed for local testing.
    2. **Real auth** (production, Phase 14.B): pass ``auth=`` with one of
       ``tako.compat.JwtAuth.hs256(secret)``,
       ``tako.compat.JwtAuth.rs256_from_pem(pem)``,
       ``tako.compat.OidcAuth.discover(issuer, audience)`` (async), or
       ``tako.compat.VaultAuth(addr, vault_token)``. These resolvers
       require the wheel to be built with the matching ``auth-*`` feature
       (e.g. ``maturin build --features auth-jwt``).

    Passing both ``tokens`` and ``auth`` is an error.
    """
    if not hasattr(orch, "_inner"):
        raise TypeError("orch must be a tako.SingleAgent or tako.Conductor instance")
    return _native.serve_openai_py(
        orch._inner,
        host=host,
        port=port,
        tokens=tokens,
        auth=auth,
        models=models,
    )


def shutdown_openai() -> None:
    """Stop the running compat server. Idempotent."""
    _native.shutdown_compat_py()


# Phase 14.B — re-export the new auth resolver pyclasses when the
# wheel was built with the matching feature. Importing names that
# don't exist in `_native` yields `AttributeError`, so guard each
# `getattr` so users can `import tako.compat` even from a slim wheel.
JwtAuth = getattr(_native, "JwtAuth", None)
OidcAuth = getattr(_native, "OidcAuth", None)
VaultAuth = getattr(_native, "VaultAuth", None)


__all__ = ["serve_openai", "shutdown_openai", "JwtAuth", "OidcAuth", "VaultAuth"]
