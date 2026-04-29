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

    Optional ``trust_root`` and ``rekor_public_key_pem`` kwargs (Phase 6,
    v0.7.0) extend the v0.6.0 leaf-cert + identity check with full
    chain-of-trust validation against an operator-pinned Fulcio root and
    Rekor SET verification against an operator-pinned Rekor public key.
    Both default to ``None`` so existing v0.6.0 callers keep working.
    """

    _native: Any

    def __init__(
        self,
        issuer: str,
        san: str,
        *,
        san_is_regex: bool = False,
        trust_root: TrustRoot | None = None,
        rekor_public_key_pem: bytes | None = None,
    ) -> None:
        tr_native = trust_root._native if trust_root is not None else None
        self._native = _native.KeylessVerifier(
            issuer,
            san,
            san_is_regex=san_is_regex,
            trust_root=tr_native,
            rekor_public_key_pem=rekor_public_key_pem,
        )

    def verify_bundle(self, manifest: bytes, bundle: bytes) -> Catalogue:
        """Check the bundle against ``manifest`` and return the parsed
        :class:`Catalogue`. Raises ``ValueError`` on mismatch.
        """
        server, tools_json = self._native.verify_bundle(manifest, bundle)
        tools = [ToolSchema.model_validate(t) for t in json.loads(tools_json)]
        return Catalogue(server=server, tools=tools)

    def verify_protobuf_bundle(
        self,
        manifest: bytes,
        protobuf_bundle: bytes,
    ) -> Catalogue:
        """Verify a cosign protobuf-bundle (Phase 7.C).

        ``protobuf_bundle`` is the wire-format output of
        ``cosign sign-blob --bundle out.pb`` (the Sigstore protobuf-specs
        ``Bundle`` v1 message). This method decodes that into the
        JSON-shaped bundle :meth:`verify_bundle` consumes and runs the
        same identity / signature / chain / Rekor pipeline.

        Raises ``ValueError`` on any mismatch and ``AttributeError``
        if the wheel was not built with the ``sigstore-protobuf``
        feature.
        """
        if not hasattr(self._native, "verify_protobuf_bundle"):
            raise AttributeError(
                "tako wheel built without `sigstore-protobuf`; rebuild with "
                "`maturin develop --features sigstore-protobuf` to use this method"
            )
        server, tools_json = self._native.verify_protobuf_bundle(manifest, protobuf_bundle)
        tools = [ToolSchema.model_validate(t) for t in json.loads(tools_json)]
        return Catalogue(server=server, tools=tools)

    def __repr__(self) -> str:
        return repr(self._native)


class TrustRoot:
    """Operator-pinned trust anchors for chain-of-trust validation.

    Build from inline PEM bytes:

    .. code-block:: python

        tr = tako.sigstore.TrustRoot(roots_pem, intermediates_pem)

    or from filesystem paths:

    .. code-block:: python

        tr = tako.sigstore.TrustRoot.from_paths(
            "fulcio.crt.pem",
            "fulcio_intermediate.crt.pem",
        )

    Pass the result to :class:`KeylessVerifier` via the ``trust_root=``
    kwarg. The verifier walks the bundle's leaf + intermediates chain
    upward and requires it to terminate at one of the pinned roots.
    """

    _native: Any

    def __init__(
        self,
        roots_pem: bytes,
        intermediates_pem: bytes | None = None,
    ) -> None:
        self._native = _native.TrustRoot(roots_pem, intermediates_pem)

    @classmethod
    def from_paths(
        cls,
        roots_path: str,
        intermediates_path: str | None = None,
    ) -> TrustRoot:
        """Load both PEM blocks from filesystem paths.

        ``intermediates_path`` may be ``None`` when the Fulcio deployment
        issues directly from a root.
        """
        instance = cls.__new__(cls)
        instance._native = _native.TrustRoot.from_paths(roots_path, intermediates_path)
        return instance

    def __repr__(self) -> str:
        return repr(self._native)


__all__ = ["Catalogue", "CatalogueVerifier", "KeylessVerifier", "TrustRoot"]
