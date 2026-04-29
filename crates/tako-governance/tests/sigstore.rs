//! End-to-end CatalogueVerifier tests.
//!
//! Generates an ECDSA-P256 keypair at test time using `sigstore`'s own
//! signing primitives so the fixtures are reproducible without `cosign`
//! installed and without committing key material to the repo.
#![cfg(feature = "sigstore")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::json;
use sigstore::crypto::SigningScheme;
use tako_governance::sigstore::{Catalogue, CatalogueVerifier};

/// Build an ECDSA-P256 signer + the matching PEM-encoded verification
/// key. Wraps the cosign-equivalent flow.
fn fresh_keypair() -> (sigstore::crypto::SigStoreSigner, String) {
    let signer = SigningScheme::ECDSA_P256_SHA256_ASN1
        .create_signer()
        .unwrap();
    let pem = signer
        .to_sigstore_keypair()
        .unwrap()
        .public_key_to_pem()
        .unwrap();
    (signer, pem)
}

fn sample_manifest() -> Vec<u8> {
    let body = json!({
        "server": "https://mcp.example.com",
        "tools": [
            {
                "name": "weather.lookup",
                "description": "Look up the current weather for a city.",
                "input_schema": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"]
                }
            },
            {
                "name": "search.web",
                "description": "Run a web search.",
                "input_schema": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }
            }
        ]
    });
    serde_json::to_vec(&body).unwrap()
}

#[test]
fn verifies_well_formed_signed_catalogue_raw_signature() {
    let (signer, pem) = fresh_keypair();
    let manifest = sample_manifest();
    let signature = signer.sign(&manifest).unwrap();

    let verifier = CatalogueVerifier::from_pem(pem.as_bytes()).unwrap();
    let catalogue: Catalogue = verifier.verify(&manifest, &signature).unwrap();

    assert_eq!(catalogue.server.as_deref(), Some("https://mcp.example.com"));
    assert_eq!(catalogue.tools.len(), 2);
    assert_eq!(catalogue.tools[0].name, "weather.lookup");
    assert_eq!(catalogue.tools[1].name, "search.web");
}

#[test]
fn verifies_base64_signature_form() {
    // `cosign sign-blob` writes base64 to stdout / `--output-signature`,
    // so the verifier should accept that without manual decoding.
    let (signer, pem) = fresh_keypair();
    let manifest = sample_manifest();
    let raw = signer.sign(&manifest).unwrap();
    let b64 = B64.encode(&raw);

    let verifier = CatalogueVerifier::from_pem(pem.as_bytes()).unwrap();
    let catalogue = verifier.verify(&manifest, b64.as_bytes()).unwrap();
    assert_eq!(catalogue.tools.len(), 2);
}

#[test]
fn rejects_tampered_manifest() {
    let (signer, pem) = fresh_keypair();
    let manifest = sample_manifest();
    let signature = signer.sign(&manifest).unwrap();

    // Flip a byte in the manifest after signing.
    let mut tampered = manifest.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;

    let verifier = CatalogueVerifier::from_pem(pem.as_bytes()).unwrap();
    let err = verifier.verify(&tampered, &signature).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("signature invalid"),
        "expected signature-invalid error, got: {msg}"
    );
}

#[test]
fn rejects_signature_from_a_different_key() {
    let (_signer1, pem1) = fresh_keypair();
    let (signer2, _pem2) = fresh_keypair();
    let manifest = sample_manifest();
    let signature = signer2.sign(&manifest).unwrap();

    let verifier = CatalogueVerifier::from_pem(pem1.as_bytes()).unwrap();
    let err = verifier.verify(&manifest, &signature).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("signature invalid"),
        "expected signature-invalid error, got: {msg}"
    );
}

#[test]
fn rejects_malformed_pem() {
    let err = CatalogueVerifier::from_pem(b"not-a-pem").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("invalid public key"),
        "expected pem-parse error, got: {msg}"
    );
}

#[test]
fn rejects_non_json_manifest_after_valid_signature() {
    let (signer, pem) = fresh_keypair();
    let payload = b"this is not json";
    let signature = signer.sign(payload).unwrap();

    let verifier = CatalogueVerifier::from_pem(pem.as_bytes()).unwrap();
    let err = verifier.verify(payload, &signature).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("catalogue parse"),
        "expected catalogue-parse error, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Keyless verifier tests (Phase 5).
// ---------------------------------------------------------------------------

mod keyless {
    use super::sample_manifest;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use rcgen::{
        CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose, KeyPair,
        KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
    };
    use sigstore::crypto::{CosignVerificationKey, Signature, SigningScheme};
    use tako_governance::{IdentityPolicy, KeylessBundle, KeylessVerifier};
    use time::OffsetDateTime;
    use x509_cert::Certificate;
    use x509_cert::der::{DecodePem, Encode};

    /// Fulcio v1 OIDC issuer extension OID (`1.3.6.1.4.1.57264.1.1`).
    const FULCIO_OIDC_ISSUER_V1: [u64; 9] = [1, 3, 6, 1, 4, 1, 57264, 1, 1];

    struct LeafFixture {
        cert_pem: String,
        signing_key: CosignVerificationKey,
        signer_keypair: KeyPair,
    }

    /// Build a Fulcio-style ECDSA-P256 leaf cert with embedded SAN +
    /// OIDC issuer extension + Code Signing EKU.
    fn build_leaf(issuer_uri: &str, san_uri: &str) -> LeafFixture {
        // Self-issued for the test: in production this would be Fulcio.
        // Chain validation against the Fulcio root is out-of-scope for
        // the v0.6.0 KeylessVerifier — operators pre-validate via cosign.
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();

        let mut params = CertificateParams::default();
        params
            .distinguished_name
            .push(DnType::CommonName, "tako-test-leaf");
        params.subject_alt_names = vec![SanType::URI(san_uri.try_into().unwrap())];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::CodeSigning];
        params.not_before = OffsetDateTime::now_utc() - time::Duration::minutes(5);
        params.not_after = OffsetDateTime::now_utc() + time::Duration::minutes(60);
        // Fulcio OIDC issuer extension (v1): IA5String holding the URI
        // bytes. rcgen's CustomExtension takes raw bytes — we pass the
        // URI bytes directly (no DER wrapping), matching Fulcio v1.
        let oid_iter: Vec<u64> = FULCIO_OIDC_ISSUER_V1.to_vec();
        let mut oidc_ext =
            CustomExtension::from_oid_content(&oid_iter, issuer_uri.as_bytes().to_vec());
        oidc_ext.set_criticality(false);
        params.custom_extensions = vec![oidc_ext];

        let cert = params.self_signed(&key_pair).unwrap();
        let cert_pem = cert.pem();

        // Re-derive the public key from the parsed cert so signature
        // verification round-trips through the same path the verifier uses.
        let parsed = Certificate::from_pem(&cert_pem).unwrap();
        let spki_der = parsed
            .tbs_certificate
            .subject_public_key_info
            .to_der()
            .unwrap();
        let signing_key =
            CosignVerificationKey::from_der(&spki_der, &SigningScheme::ECDSA_P256_SHA256_ASN1)
                .unwrap();

        LeafFixture {
            cert_pem,
            signing_key,
            signer_keypair: key_pair,
        }
    }

    /// Sign `manifest` with the leaf's private key (raw P-256 ECDSA over
    /// SHA-256). rcgen's `KeyPair` exposes its private key as DER; we
    /// route through `sigstore`'s signer to keep the test path identical
    /// to what `cosign sign-blob --key` would emit.
    fn sign_manifest(fixture: &LeafFixture, manifest: &[u8]) -> Vec<u8> {
        // `KeyPair::serialize_der` returns PKCS#8 DER. Round-trip through
        // sigstore's SigStoreKeyPair to get a SigStoreSigner.
        let pkcs8_der = fixture.signer_keypair.serialize_der();
        let sigstore_kp =
            sigstore::crypto::signing_key::SigStoreKeyPair::from_der(&pkcs8_der).unwrap();
        let signer = sigstore_kp
            .to_sigstore_signer(&SigningScheme::ECDSA_P256_SHA256_ASN1)
            .unwrap();
        let sig = signer.sign(manifest).unwrap();
        // Sanity: the cert's pubkey verifies its own signature.
        fixture
            .signing_key
            .verify_signature(Signature::Raw(&sig), manifest)
            .expect("self-check: signature must verify with cert's pubkey");
        sig
    }

    fn build_bundle(fixture: &LeafFixture, manifest: &[u8]) -> Vec<u8> {
        let sig = sign_manifest(fixture, manifest);
        let bundle = KeylessBundle {
            leaf_cert_pem: fixture.cert_pem.clone(),
            signature_b64: B64.encode(&sig),
        };
        serde_json::to_vec(&bundle).unwrap()
    }

    #[test]
    fn verifies_well_formed_keyless_bundle() {
        let manifest = sample_manifest();
        let fixture = build_leaf(
            "https://token.actions.githubusercontent.com",
            "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main",
        );
        let bundle = build_bundle(&fixture, &manifest);

        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://token.actions.githubusercontent.com",
            "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main",
        ));
        let cat = verifier.verify_bundle(&manifest, &bundle).unwrap();
        assert_eq!(cat.tools.len(), 2);
    }

    #[test]
    fn rejects_wrong_issuer() {
        let manifest = sample_manifest();
        let fixture = build_leaf(
            "https://token.actions.githubusercontent.com",
            "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main",
        );
        let bundle = build_bundle(&fixture, &manifest);

        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://accounts.google.com",
            "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main",
        ));
        let err = verifier.verify_bundle(&manifest, &bundle).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("OIDC issuer"),
            "expected issuer-mismatch error, got: {msg}"
        );
    }

    #[test]
    fn rejects_wrong_san() {
        let manifest = sample_manifest();
        let fixture = build_leaf(
            "https://token.actions.githubusercontent.com",
            "https://github.com/example/repo/.github/workflows/release.yml@refs/heads/main",
        );
        let bundle = build_bundle(&fixture, &manifest);

        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://token.actions.githubusercontent.com",
            "https://github.com/example/repo/.github/workflows/staging.yml@refs/heads/main",
        ));
        let err = verifier.verify_bundle(&manifest, &bundle).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("SAN"),
            "expected SAN-mismatch error, got: {msg}"
        );
    }

    #[test]
    fn san_regex_match() {
        let manifest = sample_manifest();
        let fixture = build_leaf(
            "https://token.actions.githubusercontent.com",
            "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main",
        );
        let bundle = build_bundle(&fixture, &manifest);

        let policy = IdentityPolicy::regex(
            "https://token.actions.githubusercontent.com",
            r"^https://github\.com/tako-ai/tako-ai-core/.+@refs/heads/main$",
        )
        .unwrap();
        let verifier = KeylessVerifier::new(policy);
        let cat = verifier.verify_bundle(&manifest, &bundle).unwrap();
        assert_eq!(cat.tools.len(), 2);
    }

    #[test]
    fn rejects_tampered_manifest() {
        let manifest = sample_manifest();
        let fixture = build_leaf(
            "https://token.actions.githubusercontent.com",
            "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main",
        );
        let bundle = build_bundle(&fixture, &manifest);

        let mut tampered = manifest.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;

        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://token.actions.githubusercontent.com",
            "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main",
        ));
        let err = verifier.verify_bundle(&tampered, &bundle).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("signature invalid"),
            "expected signature-invalid error, got: {msg}"
        );
    }

    #[test]
    fn rejects_malformed_bundle() {
        let manifest = sample_manifest();
        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://token.actions.githubusercontent.com",
            "x",
        ));
        let err = verifier.verify_bundle(&manifest, b"not-json").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("bundle parse"),
            "expected bundle-parse error, got: {msg}"
        );
    }
}
