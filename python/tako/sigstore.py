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

    ``rekor_min_tree_size`` (Phase 9.B, v0.10.0) seeds the
    trust-on-first-use freshness anchor over the Rekor checkpoint's
    ``tree_size``. Any subsequent bundle whose checkpoint reports a
    smaller value is rejected as a log rollback. Operators load this
    from a persisted state file at startup; the verifier itself is
    in-memory. Read the high-water mark back via
    :meth:`rekor_max_tree_size` after each verify to write it out.
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
        rekor_min_tree_size: int | None = None,
    ) -> None:
        tr_native = trust_root._native if trust_root is not None else None
        self._native = _native.KeylessVerifier(
            issuer,
            san,
            san_is_regex=san_is_regex,
            trust_root=tr_native,
            rekor_public_key_pem=rekor_public_key_pem,
            rekor_min_tree_size=rekor_min_tree_size,
        )

    def rekor_max_tree_size(self) -> int:
        """Phase 9.B — current high-water mark on the Rekor checkpoint
        freshness anchor. Returns ``0`` when no checkpoint has been
        observed and no seed value was set at construction.
        """
        return int(self._native.rekor_max_tree_size())

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


class JsonStateStore:
    """On-disk JSON persistence for the :class:`KeylessVerifier` Rekor
    checkpoint freshness anchor (Phase 10.A, v0.11.0).

    Phase 9.B added the in-memory anchor on :class:`KeylessVerifier`;
    this helper persists the high-water mark across process restarts.
    Crash-safe via the standard temp-then-rename pattern, so an
    interrupted save never leaves a corrupt anchor file.

    Wire shape on disk:

    .. code-block:: json

        { "rekor_min_tree_size": 4711 }

    **Confidentiality of the state file (Phase 11.A H2, v0.12.0):**
    on Unix, :meth:`save` ``chmod``s the resulting file to ``0o600``
    after the atomic replace, so a co-tenant on the same host cannot
    silently downgrade ``rekor_min_tree_size`` and re-enable rollback
    acceptance. On Windows the chmod is a no-op and the operator must
    constrain access via NTFS ACLs on the parent directory.

    Operators should additionally place the state file under a
    directory created with ``umask 077`` (or its Windows ACL
    equivalent) so the parent directory itself is not world-readable.
    See ``examples/23_state_store.py`` for a complete illustration.

    Typical operator pattern::

        import os
        os.umask(0o077)  # parent dir + state file land 0700 / 0600

        store = tako.sigstore.JsonStateStore("/var/lib/tako/rekor.json")
        verifier = store.seed(
            tako.sigstore.KeylessVerifier(
                issuer="https://accounts.example.com",
                san="ci@example.com",
                rekor_public_key_pem=rekor_pem,
            )
        )
        # ... verify bundles ...
        store.persist(verifier)
    """

    _native: Any

    def __init__(self, path: str) -> None:
        self._native = _native.JsonStateStore(path)

    @property
    def path(self) -> str:
        """Filesystem path backing this store."""
        return str(self._native.path())

    def load(self) -> int:
        """Read the persisted ``rekor_min_tree_size``. Returns ``0``
        when the file does not exist (first-boot semantics)."""
        return int(self._native.load())

    def save(self, n: int) -> None:
        """Write ``n`` as the new high-water mark via an atomic
        ``write-temp-then-rename``."""
        self._native.save(int(n))

    def seed(self, verifier: KeylessVerifier) -> KeylessVerifier:
        """Apply the persisted anchor to ``verifier`` and return it.
        Mutates the verifier's interior atomic state in place; the
        returned reference is the same object (for chainable assignment).
        """
        verifier._native = self._native.seed(verifier._native)
        return verifier

    def persist(self, verifier: KeylessVerifier) -> None:
        """Read ``verifier.rekor_max_tree_size()`` and write it via
        :meth:`save`."""
        self._native.persist(verifier._native)

    def __repr__(self) -> str:
        return repr(self._native)


class RedisStateStore:
    """Redis-backed persistence for the :class:`KeylessVerifier` Rekor
    checkpoint freshness anchor in multi-replica deployments
    (Phase 13.A, v0.14.0).

    :class:`JsonStateStore` is single-process: each replica owns an
    independent file and a slow replica can silently advance its local
    water-mark below another replica's. ``RedisStateStore`` keeps a
    single shared key in Redis. Cross-replica safety lives in a small
    Lua script enforcing **monotonic** write so a slow replica cannot
    clobber a higher water-mark with a stale value.
    Construct asynchronously via :meth:`connect`.

    Only available when the wheel is built with the ``redis`` feature
    (``maturin build --features redis``).

    Typical operator pattern (multi-replica)::

        store = await tako.sigstore.RedisStateStore.connect(
            "redis://redis.internal:6379",
        )
        verifier = await store.seed(
            tako.sigstore.KeylessVerifier(
                issuer="https://accounts.example.com",
                san="ci@example.com",
                rekor_public_key_pem=rekor_pem,
            )
        )
        # ... verify bundles ...
        await store.persist(verifier)
    """

    _native: Any

    def __init__(self) -> None:
        # Construction goes through `connect`; this constructor exists
        # only so users who hold a ready instance can subclass.
        self._native = None  # type: ignore[assignment]

    @classmethod
    async def connect(
        cls, url: str, key: str | None = None
    ) -> RedisStateStore:
        """Connect to a Redis URL and return a ready store.

        ``url`` accepts either ``redis://`` or ``rediss://`` (TLS).
        ``key`` overrides the default
        ``"tako:sigstore:rekor_min_tree_size"``.
        """
        instance = cls.__new__(cls)
        instance._native = await _native.RedisStateStore.connect(url, key)
        return instance

    @property
    def key(self) -> str:
        """The redis key backing this store."""
        return str(self._native.key())

    async def load(self) -> int:
        """Read the persisted ``rekor_min_tree_size``. Returns ``0``
        when the key does not exist (first-boot semantics)."""
        return int(await self._native.load())

    async def save(self, n: int) -> None:
        """Persist ``n`` as the new high-water mark. The redis Lua
        script enforces a monotonic compare so a stale write is
        silently dropped (the next :meth:`load` returns the higher
        existing value)."""
        await self._native.save(int(n))

    async def seed(self, verifier: KeylessVerifier) -> KeylessVerifier:
        """Apply the persisted anchor to ``verifier`` and return it.
        Mutates the verifier's interior atomic state in place; the
        returned reference is the same object (for chainable assignment).
        """
        verifier._native = await self._native.seed(verifier._native)
        return verifier

    async def persist(self, verifier: KeylessVerifier) -> None:
        """Read ``verifier.rekor_max_tree_size()`` and write it via
        :meth:`save`."""
        await self._native.persist(verifier._native)

    def __repr__(self) -> str:
        return repr(self._native)


__all__ = [
    "Catalogue",
    "CatalogueVerifier",
    "JsonStateStore",
    "KeylessVerifier",
    "RedisStateStore",
    "TrustRoot",
]
