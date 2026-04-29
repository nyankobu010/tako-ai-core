"""Sigstore tool-catalogue verification (Phase 4.E + 4.G).

The :class:`CatalogueVerifier` checks a cosign-style signature over a
JSON catalogue of allowed MCP :class:`tako.ToolSchema` entries. Pass the
returned :class:`Catalogue.tools` straight to a transport's tool list to
gate which tools the orchestrator is willing to invoke.

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
    side. Trust model is **keyed** (pinned ``cosign.pub``); keyless
    Fulcio + Rekor verification arrives in a follow-up phase.
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


__all__ = ["Catalogue", "CatalogueVerifier"]
