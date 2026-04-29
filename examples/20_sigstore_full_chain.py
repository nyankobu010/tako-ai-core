"""Phase 6.D + 6.E: full chain-of-trust + Rekor SET verification.

Builds a three-tier cert hierarchy (root → intermediate → leaf), pins
the root via :class:`tako.sigstore.TrustRoot`, mints a Rekor-style ECDSA
keypair as the transparency-log signer, and runs the full
:class:`tako.sigstore.KeylessVerifier` pipeline:

1. Identity policy (OIDC issuer + SAN) on the leaf cert.
2. Chain validation against the pinned root.
3. Rekor SET verification against the pinned Rekor public key.

In production the root + intermediate PEMs come from the Sigstore
public-good trust root (or an internal Fulcio deployment); the Rekor
public key is the public-good Rekor signing key.

Run with the ``sigstore`` feature enabled::

    maturin develop --release --features sigstore
    python examples/20_sigstore_full_chain.py
"""

from __future__ import annotations

import base64
import datetime
import json

import tako


def _build_full_bundle(
    *,
    issuer_uri: str,
    san_uri: str,
    manifest: bytes,
) -> tuple[bytes, bytes, bytes]:
    """Returns ``(root_pem, rekor_public_pem, bundle_json)``."""
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import (
        ExtendedKeyUsageOID,
        NameOID,
        ObjectIdentifier,
    )

    fulcio_oidc_issuer_v1 = ObjectIdentifier("1.3.6.1.4.1.57264.1.1")
    now = datetime.datetime.now(datetime.timezone.utc)
    near, far = now - datetime.timedelta(minutes=5), now + datetime.timedelta(hours=1)

    root_sk = ec.generate_private_key(ec.SECP256R1())
    root_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "demo-root")])
    root_cert = (
        x509.CertificateBuilder()
        .subject_name(root_name)
        .issuer_name(root_name)
        .public_key(root_sk.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(near)
        .not_valid_after(far)
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
        .sign(private_key=root_sk, algorithm=hashes.SHA256())
    )

    inter_sk = ec.generate_private_key(ec.SECP256R1())
    inter_cert = (
        x509.CertificateBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "demo-int")]))
        .issuer_name(root_cert.subject)
        .public_key(inter_sk.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(near)
        .not_valid_after(far)
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
        .sign(private_key=root_sk, algorithm=hashes.SHA256())
    )

    leaf_sk = ec.generate_private_key(ec.SECP256R1())
    leaf_cert = (
        x509.CertificateBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "demo-leaf")]))
        .issuer_name(inter_cert.subject)
        .public_key(leaf_sk.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(near)
        .not_valid_after(far)
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
        .sign(private_key=inter_sk, algorithm=hashes.SHA256())
    )

    sig = leaf_sk.sign(manifest, ec.ECDSA(hashes.SHA256()))

    # Rekor signs the canonical entry JSON.
    rekor_sk = ec.generate_private_key(ec.SECP256R1())
    body_b64 = base64.b64encode(b"demo-rekor-body").decode()
    log_id = "0" * 64
    log_index = 1
    integrated_time = int(now.timestamp())
    canonical = (
        '{"body":"' + body_b64 + '",'
        '"integratedTime":' + str(integrated_time) + ","
        '"logID":"' + log_id + '",'
        '"logIndex":' + str(log_index) + "}"
    )
    set_sig = rekor_sk.sign(canonical.encode(), ec.ECDSA(hashes.SHA256()))

    bundle = {
        "leaf_cert_pem": leaf_cert.public_bytes(serialization.Encoding.PEM).decode(),
        "signature_b64": base64.b64encode(sig).decode(),
        "chain_pem": inter_cert.public_bytes(serialization.Encoding.PEM).decode(),
        "rekor": {
            "log_index": log_index,
            "log_id": log_id,
            "integrated_time": integrated_time,
            "canonicalized_body": body_b64,
            "set_b64": base64.b64encode(set_sig).decode(),
        },
    }
    rekor_pem = rekor_sk.public_key().public_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PublicFormat.SubjectPublicKeyInfo,
    )
    root_pem = root_cert.public_bytes(serialization.Encoding.PEM)
    return root_pem, rekor_pem, json.dumps(bundle).encode()


def main() -> None:
    manifest = json.dumps(
        {
            "server": "https://mcp.example.com",
            "tools": [
                {
                    "name": "weather.lookup",
                    "description": "Look up the weather.",
                    "input_schema": {"type": "object"},
                }
            ],
        }
    ).encode()
    issuer = "https://token.actions.githubusercontent.com"
    san = "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main"

    root_pem, rekor_pem, bundle = _build_full_bundle(
        issuer_uri=issuer, san_uri=san, manifest=manifest
    )

    verifier = tako.sigstore.KeylessVerifier(
        issuer,
        san,
        trust_root=tako.sigstore.TrustRoot(root_pem),
        rekor_public_key_pem=rekor_pem,
    )
    catalogue = verifier.verify_bundle(manifest, bundle)
    print(f"server: {catalogue.server}")
    print(f"tools verified: {[t.name for t in catalogue.tools]}")


if __name__ == "__main__":
    main()
