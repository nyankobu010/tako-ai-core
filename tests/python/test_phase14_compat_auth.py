"""Phase 14.B — Python facade smoke for the new auth resolvers.

The Rust integration tests in
``crates/tako-compat/src/auth/{jwt,oidc,vault}.rs`` cover the
actual JWT signature verification, JWKS rotation, and Vault KV v2
lookup. This file verifies the Python-facing surface:

- ``tako.compat.JwtAuth.hs256(secret)`` constructs cleanly and is a
  valid ``auth=`` argument to ``serve_openai``.
- Passing both ``tokens`` and ``auth`` is rejected with a clear error.
- ``OidcAuth`` and ``VaultAuth`` re-exports exist when the wheel was
  built with the matching feature; otherwise they are ``None`` (the
  facade gracefully degrades).

These tests run against a wheel built with
``maturin develop --features "auth-jwt auth-oidc auth-vault"``.
On a wheel built without those features, the JWT-construction tests
skip themselves.
"""

from __future__ import annotations

import pytest
from tako import _native, compat


@pytest.mark.skipif(
    compat.JwtAuth is None,
    reason="wheel built without the auth-jwt feature",
)
def test_jwt_auth_hs256_constructs() -> None:
    auth = compat.JwtAuth.hs256(b"super-secret-test-secret-32-chars")
    assert auth is not None
    # Re-export should match the native type.
    assert type(auth).__name__ == "JwtAuth"


@pytest.mark.skipif(
    compat.JwtAuth is None,
    reason="wheel built without the auth-jwt feature",
)
def test_jwt_auth_with_audience_and_claims_constructs() -> None:
    auth = compat.JwtAuth.hs256(
        b"super-secret-test-secret-32-chars",
        audience="tako-api",
        issuer="https://idp.example.com",
        tenant_claim="tenant",
        user_claim="uid",
        roles_claim="groups",
    )
    assert auth is not None


@pytest.mark.skipif(
    compat.VaultAuth is None,
    reason="wheel built without the auth-vault feature",
)
def test_vault_auth_constructs() -> None:
    # Construction does NOT actually connect to Vault — the client is
    # lazy. So no Vault server is required for this smoke.
    auth = compat.VaultAuth("http://127.0.0.1:8200", "dev-token")
    assert auth is not None


def test_jwt_oidc_vault_re_exports_have_known_value() -> None:
    """At least one of the auth pyclasses should be present (or all
    None on a slim wheel). The ``getattr`` re-export shouldn't crash.
    """
    # Just confirm the attributes exist on the module.
    assert hasattr(compat, "JwtAuth")
    assert hasattr(compat, "OidcAuth")
    assert hasattr(compat, "VaultAuth")


@pytest.mark.skipif(
    compat.JwtAuth is None,
    reason="wheel built without the auth-jwt feature",
)
def test_serve_openai_rejects_both_tokens_and_auth() -> None:
    """Passing both `tokens` and `auth` is an error — operators must
    pick one mode. Verified at the PyO3 boundary so a typo doesn't
    produce a confusingly silent fallback to `tokens` only.
    """
    import tako

    auth = compat.JwtAuth.hs256(b"super-secret-test-secret-32-chars")
    fake_provider = tako.providers.Fake(canned_text="x")
    orch = tako.SingleAgent(provider=fake_provider)
    with pytest.raises(ValueError, match=r"tokens.*auth"):
        compat.serve_openai(
            orch,
            host="127.0.0.1",
            port=0,
            tokens={"my-token": ("acme", "alice")},
            auth=auth,
        )


def test_native_module_exposes_jwt_auth_when_feature_enabled() -> None:
    """Smoke: the native module's ``JwtAuth`` symbol exists iff the
    wheel was built with the matching feature.
    """
    has_native = hasattr(_native, "JwtAuth")
    has_facade = compat.JwtAuth is not None
    assert has_native == has_facade
