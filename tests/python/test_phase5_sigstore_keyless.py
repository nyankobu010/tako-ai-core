"""Smoke tests for the Phase 5 Sigstore keyless verifier.

Mirrors ``test_phase4_facades.py``'s pattern: each test auto-skips when
the wheel is built without the ``sigstore`` feature. Generates the leaf
certificate at test time with the ``cryptography`` library (already in
the ``dev`` extra) so no fixtures are committed to the repo.
"""

from __future__ import annotations

import base64
import datetime
import json

import pytest
import tako
from tako import _native


def _has(name: str) -> bool:
    return hasattr(_native, name)


def _build_leaf_and_signature(
    *,
    issuer_uri: str,
    san_uri: str,
    manifest: bytes,
) -> tuple[bytes, bytes]:
    """Mint a Fulcio-style leaf cert + ECDSA-P256 signature over `manifest`.

    Returns ``(bundle_json_bytes, raw_signature_bytes)``. Bundle JSON
    matches the ``KeylessBundle`` format the Rust verifier expects:
    ``{"leaf_cert_pem": "...", "signature_b64": "..."}``.
    """
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import (
        ExtendedKeyUsageOID,
        NameOID,
        ObjectIdentifier,
    )

    sk = ec.generate_private_key(ec.SECP256R1())
    fulcio_oidc_issuer_v1 = ObjectIdentifier("1.3.6.1.4.1.57264.1.1")

    now = datetime.datetime.now(datetime.timezone.utc)
    builder = (
        x509.CertificateBuilder()
        .subject_name(
            x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "tako-test-leaf")])
        )
        .issuer_name(
            x509.Name(
                [x509.NameAttribute(NameOID.COMMON_NAME, "tako-test-leaf-self")]
            )
        )
        .public_key(sk.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(minutes=5))
        .not_valid_after(now + datetime.timedelta(minutes=60))
        .add_extension(
            x509.SubjectAlternativeName(
                [x509.UniformResourceIdentifier(san_uri)]
            ),
            critical=False,
        )
        .add_extension(
            x509.ExtendedKeyUsage([ExtendedKeyUsageOID.CODE_SIGNING]),
            critical=False,
        )
        .add_extension(
            x509.UnrecognizedExtension(
                fulcio_oidc_issuer_v1,
                issuer_uri.encode(),
            ),
            critical=False,
        )
    )
    cert = builder.sign(private_key=sk, algorithm=hashes.SHA256())
    cert_pem = cert.public_bytes(encoding=serialization.Encoding.PEM)

    signature = sk.sign(manifest, ec.ECDSA(hashes.SHA256()))
    bundle = {
        "leaf_cert_pem": cert_pem.decode(),
        "signature_b64": base64.b64encode(signature).decode(),
    }
    return json.dumps(bundle).encode(), signature


@pytest.mark.skipif(
    not _has("KeylessVerifier"),
    reason="wheel built without `sigstore` feature",
)
def test_keyless_verifies_well_formed_bundle() -> None:
    manifest = json.dumps(
        {
            "server": "https://mcp.example.com",
            "tools": [
                {
                    "name": "weather.lookup",
                    "description": "Look up the weather for a city.",
                    "input_schema": {
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                        "required": ["city"],
                    },
                }
            ],
        }
    ).encode()
    san = "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main"
    bundle, _ = _build_leaf_and_signature(
        issuer_uri="https://token.actions.githubusercontent.com",
        san_uri=san,
        manifest=manifest,
    )

    verifier = tako.sigstore.KeylessVerifier(
        "https://token.actions.githubusercontent.com",
        san,
    )
    catalogue = verifier.verify_bundle(manifest, bundle)
    assert catalogue.server == "https://mcp.example.com"
    assert len(catalogue.tools) == 1
    assert catalogue.tools[0].name == "weather.lookup"


@pytest.mark.skipif(
    not _has("KeylessVerifier"),
    reason="wheel built without `sigstore` feature",
)
def test_keyless_rejects_wrong_issuer() -> None:
    manifest = b'{"tools":[]}'
    san = "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main"
    bundle, _ = _build_leaf_and_signature(
        issuer_uri="https://token.actions.githubusercontent.com",
        san_uri=san,
        manifest=manifest,
    )

    verifier = tako.sigstore.KeylessVerifier(
        "https://accounts.google.com",
        san,
    )
    with pytest.raises(ValueError, match="OIDC issuer"):
        verifier.verify_bundle(manifest, bundle)


@pytest.mark.skipif(
    not _has("KeylessVerifier"),
    reason="wheel built without `sigstore` feature",
)
def test_keyless_san_regex_match() -> None:
    manifest = b'{"tools":[]}'
    san = "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main"
    bundle, _ = _build_leaf_and_signature(
        issuer_uri="https://token.actions.githubusercontent.com",
        san_uri=san,
        manifest=manifest,
    )

    verifier = tako.sigstore.KeylessVerifier(
        "https://token.actions.githubusercontent.com",
        r"^https://github\.com/tako-ai/tako-ai-core/.+@refs/heads/main$",
        san_is_regex=True,
    )
    catalogue = verifier.verify_bundle(manifest, bundle)
    assert catalogue.tools == []


@pytest.mark.skipif(
    not _has("KeylessVerifier"),
    reason="wheel built without `sigstore` feature",
)
def test_keyless_rejects_tampered_manifest() -> None:
    manifest = b'{"tools":[]}'
    san = "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main"
    bundle, _ = _build_leaf_and_signature(
        issuer_uri="https://token.actions.githubusercontent.com",
        san_uri=san,
        manifest=manifest,
    )

    tampered = b'{"tools":[{"name":"evil"}]}'
    verifier = tako.sigstore.KeylessVerifier(
        "https://token.actions.githubusercontent.com",
        san,
    )
    with pytest.raises(ValueError, match="signature invalid"):
        verifier.verify_bundle(tampered, bundle)
