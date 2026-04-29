"""Sigstore tool-catalogue verification (Phase 4.E + 4.G + 5.A).

Two trust models are supported, both returning the same
:class:`Catalogue` shape:

- :class:`CatalogueVerifier` — **keyed** verification. Pin a
  cosign-style public key (``cosign.pub``) and check signatures on
  catalogues signed by the matching private key.
- :class:`KeylessVerifier` — **keyless** verification. Pin an
  :class:`IdentityPolicy` (OIDC issuer + SAN match) and verify a bundle
  whose leaf certificate carries the signing identity (typically issued
  by Fulcio).

Pass the returned :class:`Catalogue.tools` straight to a transport's
tool list to gate which tools the orchestrator is willing to invoke.

Available when the wheel was built with the ``sigstore`` Cargo feature.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

from tako import _native
from tako.models import ToolSchema


@dataclass(frozen=True)
class Catalogue:
    """A verified MCP tool catalogue."""

    server: str | None
    """Free-form server identifier (``server`` field of the manifest)."""

    tools: list[ToolSchema]
    """Allowed tool schemas, ready to feed into a registry."""


class CatalogueVerifier:
    """Verify cosign-signed MCP tool catalogues against a pinned public key.

    See :func:`tako_governance::CatalogueVerifier::from_pem` on the Rust
    side. Trust model is **keyed** (pinned ``cosign.pub``); see
    :class:`KeylessVerifier` for the keyless variant that uses
    Fulcio-issued leaf certificates.
    """

    _native: Any

    def __init__(self, pem: bytes) -> None:
        self._native = _native.CatalogueVerifier(pem)

    @classmethod
    def from_pem_path(cls, path: str) -> CatalogueVerifier:
        """Load the PEM key from a filesystem path."""
        # Bypass ``__init__`` (which expects raw bytes) so the underlying
        # Rust constructor can read the file directly without a copy.
        instance = cls.__new__(cls)
        instance._native = _native.CatalogueVerifier.from_pem_path(path)
        return instance

    def verify(self, manifest: bytes, signature: bytes) -> Catalogue:
        """Check ``signature`` over ``manifest`` and return a
        :class:`Catalogue`. Raises ``ValueError`` on mismatch.

        ``signature`` may be raw bytes or the base64-encoded form
        ``cosign sign-blob`` writes — the verifier picks transparently.
        """
        server, tools_json = self._native.verify(manifest, signature)
        tools = [ToolSchema.model_validate(t) for t in json.loads(tools_json)]
        return Catalogue(server=server, tools=tools)

    def __repr__(self) -> str:
        return repr(self._native)


class KeylessVerifier:
    """Verify cosign keyless-style bundles by matching the leaf cert's
    embedded identity (OIDC issuer + SAN) against an
    :class:`IdentityPolicy`, then checking the signature using the cert's
    public key.

    The *bundle* is a small JSON wrapper with two fields:

    .. code-block:: json

        {
          "leaf_cert_pem": "-----BEGIN CERTIFICATE-----\\n...",
          "signature_b64": "MEUCIQDx..."
        }

    Operators produce it from ``cosign sign-blob`` output (see the
    ``examples/16_sigstore_keyless.py`` recipe).

    .. note::

        The v0.6.0 keyless verifier ships **leaf-cert + identity-policy +
        signature** verification. Chain-of-trust validation against the
        Fulcio root and Rekor SET verification are tracked as follow-up
        work; operators are expected to validate those pieces with
        ``cosign verify-blob`` at deploy time and ship a pre-validated
        bundle.
    """

    _native: Any

    def __init__(
        self,
        issuer: str,
        san: str,
        *,
        san_is_regex: bool = False,
    ) -> None:
        self._native = _native.KeylessVerifier(
            issuer,
            san,
            san_is_regex=san_is_regex,
        )

    def verify_bundle(self, manifest: bytes, bundle: bytes) -> Catalogue:
        """Check the bundle against ``manifest`` and return the parsed
        :class:`Catalogue`. Raises ``ValueError`` on mismatch.
        """
        server, tools_json = self._native.verify_bundle(manifest, bundle)
        tools = [ToolSchema.model_validate(t) for t in json.loads(tools_json)]
        return Catalogue(server=server, tools=tools)

    def __repr__(self) -> str:
        return repr(self._native)


__all__ = ["Catalogue", "CatalogueVerifier", "KeylessVerifier"]
