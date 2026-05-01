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

       Phase 24.B — ``OidcAuth`` gains
       ``with_introspection_mtls(cert_pem, key_pem)`` /
       ``with_introspection_mtls_combined(combined_pem)`` builders
       for the RFC 8705 mTLS introspection auth method
       (``tls_client_auth``). The auto-selector prefers
       ``tls_client_auth`` over JWT methods when an mTLS identity is
       loaded. ``with_introspection_auth_method`` accepts new
       case-insensitive aliases ``"tls_client_auth"`` /
       ``"tls-client-auth"`` / ``"mtls"``.

       Phase 25.B — ``OidcAuth`` gains
       ``with_introspection_self_signed_mtls(cert_pem, key_pem)`` /
       ``with_introspection_self_signed_mtls_combined(combined_pem)``
       builders for the RFC 8705 §2.2 self-signed mTLS variant.
       ``with_introspection_auth_method`` accepts new aliases
       ``"self_signed_tls_client_auth"`` / ``"self-signed-mtls"``
       (and kebab variants). The auto-selector prefers CA-backed
       ``tls_client_auth`` over self-signed when both are listed.
       After Phase 25 the OIDC introspection auth-method surface
       covers all six RFC 7662 §2.1 / RFC 8414 / RFC 8705 methods.

       Phase 21.B — ``tako.compat.ChainedAuth`` (always-on; no
       cargo feature gate) is a composite resolver that wraps N
       child resolvers and tries them in append order. The first
       child to return a Principal short-circuits; any error falls
       through to the next. Common pattern: ``auth=ChainedAuth().then(oidc).then(jwt)``
       to accept either an OIDC bearer or a static-key-signed JWT.
       Children may themselves be ``ChainedAuth`` instances
       (recursive composition).

       Phase 26.B — ``ChainedAuth.with_short_circuit_on_transport_error()``
       opts in to fail-fast semantics for transport errors. When
       enabled, a transport error from any child (e.g. OIDC
       issuer unreachable) halts the chain immediately instead of
       falling through to the next child. Other error variants
       (auth-decision errors like `"bad token"`) continue to fall
       through. Default behaviour preserves Phase 21
       fall-through-on-any-Err semantics.

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
# Phase 21.B — composite resolver. Always-on; children themselves
# carry whatever `auth-*` gates they were built under.
ChainedAuth = getattr(_native, "ChainedAuth", None)


__all__ = [
    "serve_openai",
    "shutdown_openai",
    "JwtAuth",
    "OidcAuth",
    "VaultAuth",
    "ChainedAuth",
]
