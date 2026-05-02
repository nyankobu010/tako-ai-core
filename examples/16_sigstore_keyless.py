"""Phase 5.A: Sigstore keyless verification of a tool catalogue.

Generates a Fulcio-style leaf cert at runtime and signs a sample
catalogue with it. In production the leaf cert + signature come from
``cosign sign-blob --new-bundle-format ...``; this script's
``_make_bundle`` helper shows the equivalent shape.

The :class:`tako.sigstore.KeylessVerifier` then enforces an identity
policy (OIDC issuer + SAN match) and verifies the signature.

Run with a wheel built using the ``sigstore`` feature::

    maturin develop --release --features sigstore
    python examples/16_sigstore_keyless.py
"""

from __future__ import annotations

import base64
import datetime
import json

import tako


def _make_bundle(*, issuer_uri: str, san_uri: str, manifest: bytes) -> bytes:
    """Mint a self-issued Fulcio-style leaf cert + ECDSA-P256 signature
    and wrap them in the JSON shape ``KeylessVerifier`` expects.

    Replace this in production: ``cosign sign-blob`` already emits the
    cert + signature; a few lines of shell rewrap them.
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
    cert = (
        x509.CertificateBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "example-leaf")]))
        .issuer_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "example-leaf")]))
        .public_key(sk.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(minutes=5))
        .not_valid_after(now + datetime.timedelta(minutes=60))
        .add_extension(
            x509.SubjectAlternativeName([x509.UniformResourceIdentifier(san_uri)]),
            critical=False,
        )
        .add_extension(
            x509.ExtendedKeyUsage([ExtendedKeyUsageOID.CODE_SIGNING]),
            critical=False,
        )
        .add_extension(
            x509.UnrecognizedExtension(fulcio_oidc_issuer_v1, issuer_uri.encode()),
            critical=False,
        )
        .sign(private_key=sk, algorithm=hashes.SHA256())
    )
    cert_pem = cert.public_bytes(encoding=serialization.Encoding.PEM)
    signature = sk.sign(manifest, ec.ECDSA(hashes.SHA256()))
    return json.dumps(
        {
            "leaf_cert_pem": cert_pem.decode(),
            "signature_b64": base64.b64encode(signature).decode(),
        }
    ).encode()


def main() -> None:
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
    issuer = "https://token.actions.githubusercontent.com"
    san = "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main"

    bundle = _make_bundle(issuer_uri=issuer, san_uri=san, manifest=manifest)

    verifier = tako.sigstore.KeylessVerifier(issuer, san)
    catalogue = verifier.verify_bundle(manifest, bundle)
    print(f"verified server: {catalogue.server!r}")
    print(f"  tools: {[t.name for t in catalogue.tools]}")


if __name__ == "__main__":
    main()
