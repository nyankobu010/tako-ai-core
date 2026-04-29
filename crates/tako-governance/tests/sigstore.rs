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
