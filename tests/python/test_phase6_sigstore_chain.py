"""Phase 6.D + 6.E — KeylessVerifier chain-of-trust + Rekor SET (Python).

Auto-skips when the wheel was built without the ``sigstore`` feature.
Generates root + intermediate + leaf certificates via the
``cryptography`` library (already in the ``dev`` extra) so no fixtures
are committed.
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


def _build_chain(
    *,
    issuer_uri: str,
    san_uri: str,
    manifest: bytes,
) -> tuple[bytes, bytes, bytes]:
    """Mint root → intermediate → leaf and sign the manifest with the leaf.

    Returns ``(root_pem, intermediate_pem, bundle_json)``.
    """
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
    far = now + datetime.timedelta(hours=1)
    near = now - datetime.timedelta(minutes=5)

    # Root CA (self-signed)
    root_sk = ec.generate_private_key(ec.SECP256R1())
    root_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "tako-test-root")])
    root_cert = (
        x509.CertificateBuilder()
        .subject_name(root_name)
        .issuer_name(root_name)
        .public_key(root_sk.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(near)
        .not_valid_after(far)
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
        .add_extension(
            x509.KeyUsage(
                digital_signature=True,
                content_commitment=False,
                key_encipherment=False,
                data_encipherment=False,
                key_agreement=False,
                key_cert_sign=True,
                crl_sign=True,
                encipher_only=False,
                decipher_only=False,
            ),
            critical=True,
        )
        .sign(private_key=root_sk, algorithm=hashes.SHA256())
    )

    # Intermediate (signed by root)
    inter_sk = ec.generate_private_key(ec.SECP256R1())
    inter_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "tako-test-intermediate")])
    inter_cert = (
        x509.CertificateBuilder()
        .subject_name(inter_name)
        .issuer_name(root_cert.subject)
        .public_key(inter_sk.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(near)
        .not_valid_after(far)
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
        .add_extension(
            x509.KeyUsage(
                digital_signature=True,
                content_commitment=False,
                key_encipherment=False,
                data_encipherment=False,
                key_agreement=False,
                key_cert_sign=True,
                crl_sign=False,
                encipher_only=False,
                decipher_only=False,
            ),
            critical=True,
        )
        .sign(private_key=root_sk, algorithm=hashes.SHA256())
    )

    # Leaf (signed by intermediate)
    leaf_sk = ec.generate_private_key(ec.SECP256R1())
    leaf_cert = (
        x509.CertificateBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "tako-test-leaf")]))
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

    leaf_pem = leaf_cert.public_bytes(encoding=serialization.Encoding.PEM)
    inter_pem = inter_cert.public_bytes(encoding=serialization.Encoding.PEM)
    root_pem = root_cert.public_bytes(encoding=serialization.Encoding.PEM)

    sig = leaf_sk.sign(manifest, ec.ECDSA(hashes.SHA256()))
    bundle = {
        "leaf_cert_pem": leaf_pem.decode(),
        "signature_b64": base64.b64encode(sig).decode(),
        "chain_pem": inter_pem.decode(),
    }
    return root_pem, inter_pem, json.dumps(bundle).encode()


@pytest.mark.skipif(
    not _has("KeylessVerifier") or not _has("TrustRoot"),
    reason="wheel built without `sigstore` feature",
)
def test_keyless_chain_validates_against_pinned_root() -> None:
    manifest = json.dumps({"server": "ex", "tools": []}).encode()
    san = "https://example.com/svc"
    root_pem, _inter, bundle = _build_chain(
        issuer_uri="https://token.actions.githubusercontent.com",
        san_uri=san,
        manifest=manifest,
    )

    trust_root = tako.sigstore.TrustRoot(root_pem)
    verifier = tako.sigstore.KeylessVerifier(
        "https://token.actions.githubusercontent.com",
        san,
        trust_root=trust_root,
    )
    catalogue = verifier.verify_bundle(manifest, bundle)
    assert catalogue.server == "ex"


@pytest.mark.skipif(
    not _has("KeylessVerifier") or not _has("TrustRoot"),
    reason="wheel built without `sigstore` feature",
)
def test_keyless_chain_rejects_unpinned_root() -> None:
    manifest = json.dumps({"server": "ex", "tools": []}).encode()
    san = "https://example.com/svc"
    _root, _inter, bundle = _build_chain(
        issuer_uri="https://token.actions.githubusercontent.com",
        san_uri=san,
        manifest=manifest,
    )
    # Build an unrelated root and pin that one instead.
    other_root, _, _ = _build_chain(
        issuer_uri="https://token.actions.githubusercontent.com",
        san_uri=san,
        manifest=manifest,
    )

    trust_root = tako.sigstore.TrustRoot(other_root)
    verifier = tako.sigstore.KeylessVerifier(
        "https://token.actions.githubusercontent.com",
        san,
        trust_root=trust_root,
    )
    with pytest.raises(ValueError):
        verifier.verify_bundle(manifest, bundle)
