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

       Phase 15.B.1 / 15.B.2 — ``VaultAuth`` gains ``with_approle``,
       ``with_kubernetes`` and ``with_kubernetes_in_pod`` static
       constructors for AppRole / Kubernetes auth-method rotation.
       ``OidcAuth`` gains ``with_introspection`` /
       ``with_introspection_uri`` builder methods for RFC 7662 token
       introspection (revocation-aware checks).

       Phase 16.B.3 — ``VaultAuth.with_namespace(ns)`` sets the
       Vault Enterprise namespace used on every KV lookup
       (``X-Vault-Namespace`` header). ``OidcAuth.with_introspection_auth_method(m)``
       selects between ``"basic"`` (default; HTTP Basic header) and
       ``"post"`` (credentials in form body) per RFC 7662 §2.1.

       Phase 17.C — ``OidcAuth.with_introspection_auth_method`` now
       accepts ``"jwt"`` / ``"client_secret_jwt"`` for the RFC 7521 /
       7523 HS256-signed client-assertion auth method.
       ``OidcAuth.with_introspection_auth_method_from_discovery()``
       auto-selects the strongest auth method advertised by the
       issuer's RFC 8414 discovery doc (preference order: JWT > Basic
       > Post).

       Phase 18.C — ``OidcAuth`` gains
       ``with_introspection_jwt_rs256_pem(pem)`` /
       ``with_introspection_jwt_es256_pem(pem)`` /
       ``with_introspection_jwt_ed25519_pem(pem)`` builders for the
       asymmetric ``private_key_jwt`` auth method (Phase 18.A; RFC
       7521 / 7523 with an RSA / EC / Ed25519 key). The auto-selector
       prefers ``private_key_jwt`` when an asymmetric key is loaded.
       ``OidcAuth.end_session_endpoint()`` and
       ``OidcAuth.build_logout_uri(id_token_hint, post_logout_redirect_uri, state)``
       expose the OIDC Session Management 1.0 logout-URL helper
       (Phase 18.B).

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
