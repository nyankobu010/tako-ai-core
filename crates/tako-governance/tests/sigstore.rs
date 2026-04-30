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
            chain_pem: None,
            rekor: None,
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

// ---------------------------------------------------------------------------
// Phase 6.D — Chain-of-trust verification.
// ---------------------------------------------------------------------------

mod chain {
    use super::sample_manifest;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use rcgen::{
        BasicConstraints, CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose,
        IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
    };
    use sigstore::crypto::SigningScheme;
    use tako_governance::sigstore::{IdentityPolicy, KeylessBundle, KeylessVerifier, TrustRoot};
    use time::OffsetDateTime;

    const FULCIO_OIDC_ISSUER_V1: [u64; 9] = [1, 3, 6, 1, 4, 1, 57264, 1, 1];

    /// Three-tier chain (root CA → intermediate CA → leaf) all using
    /// ECDSA-P256, mirroring Fulcio's deployment shape.
    struct Chain {
        root_pem: String,
        intermediate_pem: String,
        leaf_pem: String,
        leaf_keypair: KeyPair,
    }

    fn build_chain(issuer_uri: &str, san_uri: &str) -> Chain {
        let now = OffsetDateTime::now_utc();
        let later = now + time::Duration::hours(1);
        let earlier = now - time::Duration::minutes(5);

        // Root CA (self-signed).
        let root_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut root_params = CertificateParams::new(Vec::default()).unwrap();
        root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        root_params
            .distinguished_name
            .push(DnType::CommonName, "tako-test-root");
        root_params.key_usages.push(KeyUsagePurpose::KeyCertSign);
        root_params
            .key_usages
            .push(KeyUsagePurpose::DigitalSignature);
        root_params.not_before = earlier;
        root_params.not_after = later;
        let root_cert = root_params.clone().self_signed(&root_kp).unwrap();
        let root_pem = root_cert.pem();
        let root_issuer: Issuer<'static, KeyPair> = Issuer::new(root_params, root_kp);

        // Intermediate CA (signed by root).
        let inter_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut inter_params = CertificateParams::new(Vec::default()).unwrap();
        inter_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        inter_params
            .distinguished_name
            .push(DnType::CommonName, "tako-test-intermediate");
        inter_params.key_usages.push(KeyUsagePurpose::KeyCertSign);
        inter_params
            .key_usages
            .push(KeyUsagePurpose::DigitalSignature);
        inter_params.use_authority_key_identifier_extension = true;
        inter_params.not_before = earlier;
        inter_params.not_after = later;
        let inter_cert = inter_params
            .clone()
            .signed_by(&inter_kp, &root_issuer)
            .unwrap();
        let intermediate_pem = inter_cert.pem();
        let inter_issuer: Issuer<'static, KeyPair> = Issuer::new(inter_params, inter_kp);

        // Leaf signed by intermediate. Identical Fulcio extensions to the
        // existing `keyless::build_leaf`, but signed by an intermediate
        // instead of being self-signed.
        let leaf_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut leaf_params = CertificateParams::default();
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, "tako-test-leaf");
        leaf_params.subject_alt_names = vec![SanType::URI(san_uri.try_into().unwrap())];
        leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::CodeSigning];
        leaf_params.use_authority_key_identifier_extension = true;
        leaf_params.not_before = earlier;
        leaf_params.not_after = later;
        let oid_iter: Vec<u64> = FULCIO_OIDC_ISSUER_V1.to_vec();
        let mut oidc_ext =
            CustomExtension::from_oid_content(&oid_iter, issuer_uri.as_bytes().to_vec());
        oidc_ext.set_criticality(false);
        leaf_params.custom_extensions = vec![oidc_ext];
        let leaf_cert = leaf_params.signed_by(&leaf_kp, &inter_issuer).unwrap();

        Chain {
            root_pem,
            intermediate_pem,
            leaf_pem: leaf_cert.pem(),
            leaf_keypair: leaf_kp,
        }
    }

    fn sign_with_leaf(chain: &Chain, manifest: &[u8]) -> Vec<u8> {
        let pkcs8 = chain.leaf_keypair.serialize_der();
        let kp = sigstore::crypto::signing_key::SigStoreKeyPair::from_der(&pkcs8).unwrap();
        let signer = kp
            .to_sigstore_signer(&SigningScheme::ECDSA_P256_SHA256_ASN1)
            .unwrap();
        signer.sign(manifest).unwrap()
    }

    fn build_bundle_with_chain(chain: &Chain, manifest: &[u8]) -> Vec<u8> {
        let sig = sign_with_leaf(chain, manifest);
        let bundle = KeylessBundle {
            leaf_cert_pem: chain.leaf_pem.clone(),
            signature_b64: B64.encode(&sig),
            chain_pem: Some(chain.intermediate_pem.clone()),
            rekor: None,
        };
        serde_json::to_vec(&bundle).unwrap()
    }

    #[test]
    fn chain_validates_against_pinned_root() {
        let manifest = sample_manifest();
        let chain = build_chain(
            "https://token.actions.githubusercontent.com",
            "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main",
        );
        let bundle = build_bundle_with_chain(&chain, &manifest);

        let trust_root = TrustRoot::from_pem(chain.root_pem.as_bytes(), None).unwrap();
        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://token.actions.githubusercontent.com",
            "https://github.com/tako-ai/tako-ai-core/.github/workflows/release.yml@refs/heads/main",
        ))
        .with_trust_root(trust_root);
        let cat = verifier.verify_bundle(&manifest, &bundle).unwrap();
        assert_eq!(cat.tools.len(), 2);
    }

    #[test]
    fn chain_rejects_unknown_root() {
        let manifest = sample_manifest();
        let chain = build_chain(
            "https://token.actions.githubusercontent.com",
            "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main",
        );
        let bundle = build_bundle_with_chain(&chain, &manifest);

        // Pin a *different* root than the one that signed the chain.
        let other_chain = build_chain(
            "https://token.actions.githubusercontent.com",
            "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main",
        );
        let trust_root = TrustRoot::from_pem(other_chain.root_pem.as_bytes(), None).unwrap();
        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://token.actions.githubusercontent.com",
            "https://github.com/example/repo/.github/workflows/x.yml@refs/heads/main",
        ))
        .with_trust_root(trust_root);
        let err = verifier.verify_bundle(&manifest, &bundle).unwrap_err();
        let msg = format!("{err}");
        // Either path indicates the chain failed to anchor at the
        // pinned root: a missing-issuer match, or a signature-verify
        // mismatch when DNs collide but keys don't.
        assert!(
            msg.contains("unknown issuer")
                || msg.contains("chain signature invalid")
                || msg.contains("self-signed"),
            "expected chain-validation error, got: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 6.E — Rekor SET verification.
// ---------------------------------------------------------------------------

mod rekor {
    use super::sample_manifest;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use rcgen::{
        CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose, KeyPair,
        KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
    };
    use sigstore::crypto::{SigStoreSigner, SigningScheme};
    use tako_governance::sigstore::{IdentityPolicy, KeylessBundle, KeylessVerifier, RekorEntry};
    use time::OffsetDateTime;

    const FULCIO_OIDC_ISSUER_V1: [u64; 9] = [1, 3, 6, 1, 4, 1, 57264, 1, 1];

    /// Mint a Rekor-style ECDSA-P256 keypair and the matching PEM.
    fn fresh_rekor_keypair() -> (SigStoreSigner, String) {
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

    /// Reuse the test leaf-cert helper that the keyless module uses.
    fn build_leaf(issuer_uri: &str, san_uri: &str) -> (String, KeyPair) {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, "leaf");
        params.subject_alt_names = vec![SanType::URI(san_uri.try_into().unwrap())];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::CodeSigning];
        params.not_before = OffsetDateTime::now_utc() - time::Duration::minutes(5);
        params.not_after = OffsetDateTime::now_utc() + time::Duration::minutes(60);
        let oid_iter: Vec<u64> = FULCIO_OIDC_ISSUER_V1.to_vec();
        let mut ext = CustomExtension::from_oid_content(&oid_iter, issuer_uri.as_bytes().to_vec());
        ext.set_criticality(false);
        params.custom_extensions = vec![ext];
        let cert = params.self_signed(&key_pair).unwrap();
        (cert.pem(), key_pair)
    }

    fn sign_manifest(kp: &KeyPair, manifest: &[u8]) -> Vec<u8> {
        let pkcs8 = kp.serialize_der();
        let sk = sigstore::crypto::signing_key::SigStoreKeyPair::from_der(&pkcs8).unwrap();
        let signer = sk
            .to_sigstore_signer(&SigningScheme::ECDSA_P256_SHA256_ASN1)
            .unwrap();
        signer.sign(manifest).unwrap()
    }

    /// Mint a Rekor SET over the canonical entry-JSON.
    fn mint_set(
        rekor_signer: &SigStoreSigner,
        body_b64: &str,
        integrated_time: i64,
        log_id: &str,
        log_index: u64,
    ) -> String {
        let canonical = format!(
            "{{\"body\":\"{body}\",\"integratedTime\":{ts},\"logID\":\"{log_id}\",\"logIndex\":{idx}}}",
            body = body_b64,
            ts = integrated_time,
            log_id = log_id,
            idx = log_index,
        );
        let sig = rekor_signer.sign(canonical.as_bytes()).unwrap();
        B64.encode(&sig)
    }

    #[test]
    fn rekor_set_round_trips_against_pinned_key() {
        let manifest = sample_manifest();
        let (leaf_pem, leaf_kp) = build_leaf(
            "https://token.actions.githubusercontent.com",
            "https://example.com/svc",
        );
        let manifest_sig = sign_manifest(&leaf_kp, &manifest);

        let (rekor_signer, rekor_pem) = fresh_rekor_keypair();
        let body_b64 = B64.encode(b"rekor-body-fixture");
        let log_id = "c0d23d6ad406973f9559f3ba2d1ca01f84147d8ffc5b8445c224f98b9591801d".to_string();
        let log_index = 12_345u64;
        let integrated_time = 1_700_000_000_i64;
        let set_b64 = mint_set(
            &rekor_signer,
            &body_b64,
            integrated_time,
            &log_id,
            log_index,
        );

        let bundle = KeylessBundle {
            leaf_cert_pem: leaf_pem,
            signature_b64: B64.encode(&manifest_sig),
            chain_pem: None,
            rekor: Some(RekorEntry {
                log_index,
                log_id,
                integrated_time,
                canonicalized_body: body_b64,
                set_b64,
                inclusion_proof: None,
                checkpoint: None,
            }),
        };
        let bundle_bytes = serde_json::to_vec(&bundle).unwrap();

        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://token.actions.githubusercontent.com",
            "https://example.com/svc",
        ))
        .with_rekor_key(rekor_pem.as_bytes())
        .unwrap();
        let cat = verifier.verify_bundle(&manifest, &bundle_bytes).unwrap();
        assert_eq!(cat.tools.len(), 2);
    }

    #[test]
    fn rekor_set_rejects_tampered_signature() {
        let manifest = sample_manifest();
        let (leaf_pem, leaf_kp) = build_leaf(
            "https://token.actions.githubusercontent.com",
            "https://example.com/svc",
        );
        let manifest_sig = sign_manifest(&leaf_kp, &manifest);

        let (rekor_signer, rekor_pem) = fresh_rekor_keypair();
        let body_b64 = B64.encode(b"rekor-body-fixture");
        let log_id = "abcd".repeat(16);
        let log_index = 77u64;
        let integrated_time = 1_700_000_000_i64;
        let valid_set_b64 = mint_set(
            &rekor_signer,
            &body_b64,
            integrated_time,
            &log_id,
            log_index,
        );
        // Decode the SET, flip a byte in the raw signature, re-encode.
        // This corrupts the signature without breaking base64 framing.
        let mut raw = B64.decode(&valid_set_b64).unwrap();
        let mid = raw.len() / 2;
        raw[mid] ^= 0x01;
        let set_b64 = B64.encode(&raw);

        let bundle = KeylessBundle {
            leaf_cert_pem: leaf_pem,
            signature_b64: B64.encode(&manifest_sig),
            chain_pem: None,
            rekor: Some(RekorEntry {
                log_index,
                log_id,
                integrated_time,
                canonicalized_body: body_b64,
                set_b64,
                inclusion_proof: None,
                checkpoint: None,
            }),
        };
        let bundle_bytes = serde_json::to_vec(&bundle).unwrap();

        let verifier = KeylessVerifier::new(IdentityPolicy::exact(
            "https://token.actions.githubusercontent.com",
            "https://example.com/svc",
        ))
        .with_rekor_key(rekor_pem.as_bytes())
        .unwrap();
        let err = verifier
            .verify_bundle(&manifest, &bundle_bytes)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("rekor SET invalid") || msg.contains("rekor SET base64"),
            "expected rekor-set error, got: {msg}"
        );
    }
}

mod inclusion_proof {
    //! Phase 7.A — Rekor Merkle inclusion-proof verification.
    //!
    //! Each test builds a runtime Merkle tree per RFC 6962 (leaves
    //! hashed with the 0x00 prefix, internal nodes with 0x01) and
    //! exercises [`KeylessVerifier::verify_bundle`] against it.

    use super::sample_manifest;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use rcgen::{
        CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose, KeyPair,
        KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
    };
    use sha2::{Digest, Sha256};
    use sigstore::crypto::{SigStoreSigner, SigningScheme};
    use tako_governance::sigstore::{
        IdentityPolicy, KeylessBundle, KeylessVerifier, RekorEntry, RekorInclusionProof,
    };
    use time::OffsetDateTime;

    const FULCIO_OIDC_ISSUER_V1: [u64; 9] = [1, 3, 6, 1, 4, 1, 57264, 1, 1];

    fn fresh_rekor_keypair() -> (SigStoreSigner, String) {
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

    fn build_leaf(issuer_uri: &str, san_uri: &str) -> (String, KeyPair) {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, "leaf");
        params.subject_alt_names = vec![SanType::URI(san_uri.try_into().unwrap())];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::CodeSigning];
        params.not_before = OffsetDateTime::now_utc() - time::Duration::minutes(5);
        params.not_after = OffsetDateTime::now_utc() + time::Duration::minutes(60);
        let oid_iter: Vec<u64> = FULCIO_OIDC_ISSUER_V1.to_vec();
        let mut ext = CustomExtension::from_oid_content(&oid_iter, issuer_uri.as_bytes().to_vec());
        ext.set_criticality(false);
        params.custom_extensions = vec![ext];
        let cert = params.self_signed(&key_pair).unwrap();
        (cert.pem(), key_pair)
    }

    fn sign_manifest(kp: &KeyPair, manifest: &[u8]) -> Vec<u8> {
        let pkcs8 = kp.serialize_der();
        let sk = sigstore::crypto::signing_key::SigStoreKeyPair::from_der(&pkcs8).unwrap();
        let signer = sk
            .to_sigstore_signer(&SigningScheme::ECDSA_P256_SHA256_ASN1)
            .unwrap();
        signer.sign(manifest).unwrap()
    }

    fn mint_set(
        rekor_signer: &SigStoreSigner,
        body_b64: &str,
        integrated_time: i64,
        log_id: &str,
        log_index: u64,
    ) -> String {
        let canonical = format!(
            "{{\"body\":\"{body}\",\"integratedTime\":{ts},\"logID\":\"{log_id}\",\"logIndex\":{idx}}}",
            body = body_b64,
            ts = integrated_time,
            log_id = log_id,
            idx = log_index,
        );
        let sig = rekor_signer.sign(canonical.as_bytes()).unwrap();
        B64.encode(&sig)
    }

    fn leaf_hash(body: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update([0x00u8]);
        h.update(body);
        h.finalize().into()
    }

    fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update([0x01u8]);
        h.update(left);
        h.update(right);
        h.finalize().into()
    }

    /// Compute the RFC 6962 Merkle tree hash (MTH) over `leaves`. The
    /// split at each level is at the largest power of two strictly
    /// less than the subtree's leaf count.
    fn mth(leaves: &[[u8; 32]]) -> [u8; 32] {
        match leaves.len() {
            0 => panic!("empty tree"),
            1 => leaves[0],
            n => {
                let mut k: usize = 1;
                while k < n {
                    k <<= 1;
                }
                k >>= 1;
                let left = mth(&leaves[..k]);
                let right = mth(&leaves[k..]);
                node_hash(&left, &right)
            }
        }
    }

    /// Build the RFC 6962 inclusion-proof (audit path) for `index` in
    /// the tree of `leaves`. Returns the bottom-up sibling list.
    fn audit_path(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
        fn rec(leaves: &[[u8; 32]], index: usize, out: &mut Vec<[u8; 32]>) {
            if leaves.len() <= 1 {
                return;
            }
            let n = leaves.len();
            let mut k: usize = 1;
            while k < n {
                k <<= 1;
            }
            k >>= 1;
            if index < k {
                rec(&leaves[..k], index, out);
                out.push(mth(&leaves[k..]));
            } else {
                rec(&leaves[k..], index - k, out);
                out.push(mth(&leaves[..k]));
            }
        }
        let mut path = Vec::new();
        rec(leaves, index, &mut path);
        path
    }

    /// Build a canonical bundle with a fresh tree, returning the
    /// bundle JSON, the matching identity policy, and the Rekor PEM.
    fn build_bundle_with_proof(
        n_leaves: usize,
        target_index: usize,
    ) -> (Vec<u8>, IdentityPolicy, String) {
        let manifest = sample_manifest();
        let issuer = "https://token.actions.githubusercontent.com";
        let san = "https://example.com/svc";
        let (leaf_pem, leaf_kp) = build_leaf(issuer, san);
        let manifest_sig = sign_manifest(&leaf_kp, &manifest);

        let (rekor_signer, rekor_pem) = fresh_rekor_keypair();
        let log_id = "ab".repeat(32);
        let log_index = target_index as u64;
        let integrated_time = 1_700_000_000_i64;

        // Synthesise n_leaves distinct bodies; the target index's body
        // is the one we'll embed in the bundle.
        let bodies: Vec<Vec<u8>> = (0..n_leaves)
            .map(|i| format!("rekor-body-{i}").into_bytes())
            .collect();
        let leaf_hashes: Vec<[u8; 32]> = bodies.iter().map(|b| leaf_hash(b)).collect();
        let root = mth(&leaf_hashes);
        let path = audit_path(&leaf_hashes, target_index);

        let body_b64 = B64.encode(&bodies[target_index]);
        let set_b64 = mint_set(
            &rekor_signer,
            &body_b64,
            integrated_time,
            &log_id,
            log_index,
        );
        let inclusion = RekorInclusionProof {
            hashes_hex: path.iter().map(hex::encode).collect(),
            tree_size: n_leaves as u64,
            log_index,
            root_hash_hex: hex::encode(root),
        };

        let bundle = KeylessBundle {
            leaf_cert_pem: leaf_pem,
            signature_b64: B64.encode(&manifest_sig),
            chain_pem: None,
            rekor: Some(RekorEntry {
                log_index,
                log_id,
                integrated_time,
                canonicalized_body: body_b64,
                set_b64,
                inclusion_proof: Some(inclusion),
                checkpoint: None,
            }),
        };
        (
            serde_json::to_vec(&bundle).unwrap(),
            IdentityPolicy::exact(issuer, san),
            rekor_pem,
        )
    }

    #[test]
    fn round_trips_against_runtime_built_tree() {
        let manifest = sample_manifest();
        // Tree of 5 leaves; verify inclusion of index 2 (covers the
        // mixed-subtree audit-path branch).
        let (bundle_bytes, identity, rekor_pem) = build_bundle_with_proof(5, 2);
        let verifier = KeylessVerifier::new(identity)
            .with_rekor_key(rekor_pem.as_bytes())
            .unwrap();
        let cat = verifier.verify_bundle(&manifest, &bundle_bytes).unwrap();
        assert_eq!(cat.tools.len(), 2);

        // Right-edge leaf in an unbalanced tree exercises the
        // `fn == sn` branch of the verifier.
        let (bundle_bytes, identity, rekor_pem) = build_bundle_with_proof(5, 4);
        let verifier = KeylessVerifier::new(identity)
            .with_rekor_key(rekor_pem.as_bytes())
            .unwrap();
        verifier.verify_bundle(&manifest, &bundle_bytes).unwrap();
    }

    #[test]
    fn rejects_tampered_audit_path_hash() {
        let manifest = sample_manifest();
        let (bundle_bytes, identity, rekor_pem) = build_bundle_with_proof(5, 2);
        let mut bundle: KeylessBundle = serde_json::from_slice(&bundle_bytes).unwrap();
        let proof = bundle
            .rekor
            .as_mut()
            .unwrap()
            .inclusion_proof
            .as_mut()
            .unwrap();
        // Flip a hex char in the first sibling hash.
        let mut chars: Vec<char> = proof.hashes_hex[0].chars().collect();
        chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
        proof.hashes_hex[0] = chars.into_iter().collect();
        let bundle_bytes = serde_json::to_vec(&bundle).unwrap();

        let verifier = KeylessVerifier::new(identity)
            .with_rekor_key(rekor_pem.as_bytes())
            .unwrap();
        let err = verifier
            .verify_bundle(&manifest, &bundle_bytes)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("rekor inclusion proof"),
            "expected inclusion-proof error, got: {msg}"
        );
    }

    #[test]
    fn rejects_mutated_root_hash() {
        let manifest = sample_manifest();
        let (bundle_bytes, identity, rekor_pem) = build_bundle_with_proof(5, 2);
        let mut bundle: KeylessBundle = serde_json::from_slice(&bundle_bytes).unwrap();
        let proof = bundle
            .rekor
            .as_mut()
            .unwrap()
            .inclusion_proof
            .as_mut()
            .unwrap();
        // Replace the root with a hash of a constant.
        let mut h = Sha256::new();
        h.update(b"not-the-real-root");
        let bogus: [u8; 32] = h.finalize().into();
        proof.root_hash_hex = hex::encode(bogus);
        let bundle_bytes = serde_json::to_vec(&bundle).unwrap();

        let verifier = KeylessVerifier::new(identity)
            .with_rekor_key(rekor_pem.as_bytes())
            .unwrap();
        let err = verifier
            .verify_bundle(&manifest, &bundle_bytes)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("rekor inclusion proof"),
            "expected inclusion-proof error, got: {msg}"
        );
    }
}

mod checkpoint {
    //! Phase 8.C — Rekor checkpoint (`SignedNote`) verification.
    //!
    //! Each test mints a fresh Rekor ECDSA-P256 keypair, builds a
    //! 5-leaf Merkle tree, signs a SignedNote-formatted body, and
    //! exercises [`KeylessVerifier::verify_bundle`] over the
    //! resulting `KeylessBundle`. SET + inclusion proof + checkpoint
    //! all run in the same `verify_bundle` invocation.

    use super::sample_manifest;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use rcgen::{
        CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose, KeyPair,
        KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
    };
    use sha2::{Digest, Sha256};
    use sigstore::crypto::{SigStoreSigner, SigningScheme};
    use tako_governance::sigstore::{
        IdentityPolicy, KeylessBundle, KeylessVerifier, RekorCheckpoint, RekorEntry,
        RekorInclusionProof,
    };
    use time::OffsetDateTime;

    const FULCIO_OIDC_ISSUER_V1: [u64; 9] = [1, 3, 6, 1, 4, 1, 57264, 1, 1];

    fn fresh_rekor_keypair() -> (SigStoreSigner, String) {
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

    fn build_leaf(issuer_uri: &str, san_uri: &str) -> (String, KeyPair) {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, "leaf");
        params.subject_alt_names = vec![SanType::URI(san_uri.try_into().unwrap())];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::CodeSigning];
        params.not_before = OffsetDateTime::now_utc() - time::Duration::minutes(5);
        params.not_after = OffsetDateTime::now_utc() + time::Duration::minutes(60);
        let oid_iter: Vec<u64> = FULCIO_OIDC_ISSUER_V1.to_vec();
        let mut ext = CustomExtension::from_oid_content(&oid_iter, issuer_uri.as_bytes().to_vec());
        ext.set_criticality(false);
        params.custom_extensions = vec![ext];
        let cert = params.self_signed(&key_pair).unwrap();
        (cert.pem(), key_pair)
    }

    fn sign_manifest(kp: &KeyPair, manifest: &[u8]) -> Vec<u8> {
        let pkcs8 = kp.serialize_der();
        let sk = sigstore::crypto::signing_key::SigStoreKeyPair::from_der(&pkcs8).unwrap();
        let signer = sk
            .to_sigstore_signer(&SigningScheme::ECDSA_P256_SHA256_ASN1)
            .unwrap();
        signer.sign(manifest).unwrap()
    }

    fn mint_set(
        rekor_signer: &SigStoreSigner,
        body_b64: &str,
        integrated_time: i64,
        log_id: &str,
        log_index: u64,
    ) -> String {
        let canonical = format!(
            "{{\"body\":\"{body}\",\"integratedTime\":{ts},\"logID\":\"{log_id}\",\"logIndex\":{idx}}}",
            body = body_b64,
            ts = integrated_time,
            log_id = log_id,
            idx = log_index,
        );
        let sig = rekor_signer.sign(canonical.as_bytes()).unwrap();
        B64.encode(&sig)
    }

    fn leaf_hash(body: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update([0x00u8]);
        h.update(body);
        h.finalize().into()
    }

    fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update([0x01u8]);
        h.update(left);
        h.update(right);
        h.finalize().into()
    }

    fn mth(leaves: &[[u8; 32]]) -> [u8; 32] {
        match leaves.len() {
            0 => panic!("empty tree"),
            1 => leaves[0],
            n => {
                let mut k: usize = 1;
                while k < n {
                    k <<= 1;
                }
                k >>= 1;
                let left = mth(&leaves[..k]);
                let right = mth(&leaves[k..]);
                node_hash(&left, &right)
            }
        }
    }

    fn audit_path(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
        fn rec(leaves: &[[u8; 32]], index: usize, out: &mut Vec<[u8; 32]>) {
            if leaves.len() <= 1 {
                return;
            }
            let n = leaves.len();
            let mut k: usize = 1;
            while k < n {
                k <<= 1;
            }
            k >>= 1;
            if index < k {
                rec(&leaves[..k], index, out);
                out.push(mth(&leaves[k..]));
            } else {
                rec(&leaves[k..], index - k, out);
                out.push(mth(&leaves[..k]));
            }
        }
        let mut path = Vec::new();
        rec(leaves, index, &mut path);
        path
    }

    /// Mint a SignedNote-format checkpoint over `(origin, tree_size,
    /// root_hash)` using `rekor_signer` and return the populated
    /// [`RekorCheckpoint`]. The signed message is the canonical
    /// `format!("{origin}\n{tree_size}\n{root_hash_b64}\n\n")`.
    pub(super) fn mint_checkpoint(
        rekor_signer: &SigStoreSigner,
        origin: &str,
        tree_size: u64,
        root_hash_b64: &str,
        key_id: &str,
    ) -> RekorCheckpoint {
        let signed_message = format!("{origin}\n{tree_size}\n{root_hash_b64}\n\n");
        let sig = rekor_signer.sign(signed_message.as_bytes()).unwrap();
        RekorCheckpoint {
            origin: origin.to_string(),
            tree_size,
            root_hash_b64: root_hash_b64.to_string(),
            key_id: key_id.to_string(),
            signature_b64: B64.encode(&sig),
        }
    }

    /// All the pieces a checkpoint test might want to mutate before
    /// serialising the bundle. Returned by [`build_bundle_pieces`] so
    /// individual tests can swap fields and still re-sign with the
    /// same Rekor key (isolating root-mismatch from signature tamper).
    pub(super) struct BundlePieces {
        pub(super) manifest: Vec<u8>,
        pub(super) leaf_pem: String,
        pub(super) leaf_kp: KeyPair,
        pub(super) rekor_signer: SigStoreSigner,
        pub(super) rekor_pem: String,
        pub(super) identity: IdentityPolicy,
        pub(super) log_id: String,
        pub(super) log_index: u64,
        pub(super) integrated_time: i64,
        pub(super) body_b64: String,
        pub(super) inclusion: RekorInclusionProof,
        pub(super) true_root: [u8; 32],
        pub(super) n_leaves: u64,
    }

    pub(super) fn build_bundle_pieces() -> BundlePieces {
        let manifest = sample_manifest();
        let issuer = "https://token.actions.githubusercontent.com";
        let san = "https://example.com/svc";
        let (leaf_pem, leaf_kp) = build_leaf(issuer, san);

        let (rekor_signer, rekor_pem) = fresh_rekor_keypair();
        let log_id = "ab".repeat(32);
        let target_index = 2usize;
        let log_index = target_index as u64;
        let integrated_time = 1_700_000_000_i64;
        let n_leaves = 5usize;

        let bodies: Vec<Vec<u8>> = (0..n_leaves)
            .map(|i| format!("rekor-body-{i}").into_bytes())
            .collect();
        let leaf_hashes: Vec<[u8; 32]> = bodies.iter().map(|b| leaf_hash(b)).collect();
        let root = mth(&leaf_hashes);
        let path = audit_path(&leaf_hashes, target_index);

        let body_b64 = B64.encode(&bodies[target_index]);
        let inclusion = RekorInclusionProof {
            hashes_hex: path.iter().map(hex::encode).collect(),
            tree_size: n_leaves as u64,
            log_index,
            root_hash_hex: hex::encode(root),
        };

        BundlePieces {
            manifest,
            leaf_pem,
            leaf_kp,
            rekor_signer,
            rekor_pem,
            identity: IdentityPolicy::exact(issuer, san),
            log_id,
            log_index,
            integrated_time,
            body_b64,
            inclusion,
            true_root: root,
            n_leaves: n_leaves as u64,
        }
    }

    /// Assemble the [`KeylessBundle`] from the checkpoint pieces plus
    /// a custom checkpoint. Re-signs the manifest with the leaf key
    /// and mints the SET with the Rekor signer.
    pub(super) fn assemble_bundle(p: &BundlePieces, checkpoint: RekorCheckpoint) -> Vec<u8> {
        let manifest_sig = sign_manifest(&p.leaf_kp, &p.manifest);
        let set_b64 = mint_set(
            &p.rekor_signer,
            &p.body_b64,
            p.integrated_time,
            &p.log_id,
            p.log_index,
        );
        let bundle = KeylessBundle {
            leaf_cert_pem: p.leaf_pem.clone(),
            signature_b64: B64.encode(&manifest_sig),
            chain_pem: None,
            rekor: Some(RekorEntry {
                log_index: p.log_index,
                log_id: p.log_id.clone(),
                integrated_time: p.integrated_time,
                canonicalized_body: p.body_b64.clone(),
                set_b64,
                inclusion_proof: Some(p.inclusion.clone()),
                checkpoint: Some(checkpoint),
            }),
        };
        serde_json::to_vec(&bundle).unwrap()
    }

    #[test]
    fn round_trips_with_checkpoint() {
        let p = build_bundle_pieces();
        let checkpoint = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            p.n_leaves,
            &B64.encode(p.true_root),
            "rekor.sigstore.dev",
        );
        let bundle_bytes = assemble_bundle(&p, checkpoint);
        let verifier = KeylessVerifier::new(p.identity.clone())
            .with_rekor_key(p.rekor_pem.as_bytes())
            .unwrap();
        let cat = verifier.verify_bundle(&p.manifest, &bundle_bytes).unwrap();
        assert_eq!(cat.tools.len(), 2);
    }

    #[test]
    fn rejects_tampered_checkpoint_signature() {
        let p = build_bundle_pieces();
        let mut checkpoint = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            p.n_leaves,
            &B64.encode(p.true_root),
            "rekor.sigstore.dev",
        );
        // Decode the signature, flip a byte in the middle, re-encode
        // — corrupts the cryptographic content without breaking
        // base64 framing.
        let mut raw = B64.decode(&checkpoint.signature_b64).unwrap();
        let mid = raw.len() / 2;
        raw[mid] ^= 0x01;
        checkpoint.signature_b64 = B64.encode(&raw);

        let bundle_bytes = assemble_bundle(&p, checkpoint);
        let verifier = KeylessVerifier::new(p.identity.clone())
            .with_rekor_key(p.rekor_pem.as_bytes())
            .unwrap();
        let err = verifier
            .verify_bundle(&p.manifest, &bundle_bytes)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("rekor checkpoint"),
            "expected checkpoint error, got: {msg}"
        );
    }

    #[test]
    fn rejects_root_hash_disagreement_with_inclusion_proof() {
        // Mint a fully-valid checkpoint signed by the legitimate Rekor
        // key, but over a *different* root than the inclusion proof
        // claims. The signature check itself passes — only the
        // cross-check against the inclusion proof should fail.
        let p = build_bundle_pieces();
        let bogus_root: [u8; 32] = {
            let mut h = Sha256::new();
            h.update(b"not-the-real-root-mismatch");
            h.finalize().into()
        };
        assert_ne!(bogus_root, p.true_root);

        let checkpoint = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            p.n_leaves,
            &B64.encode(bogus_root),
            "rekor.sigstore.dev",
        );
        let bundle_bytes = assemble_bundle(&p, checkpoint);
        let verifier = KeylessVerifier::new(p.identity.clone())
            .with_rekor_key(p.rekor_pem.as_bytes())
            .unwrap();
        let err = verifier
            .verify_bundle(&p.manifest, &bundle_bytes)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("disagrees with inclusion proof"),
            "expected root-mismatch error, got: {msg}"
        );
    }
}

mod checkpoint_freshness {
    //! Phase 9.B — Rekor checkpoint freshness anchor (TOFU).
    //!
    //! Verifies that a successful checkpoint observation atomically
    //! advances the verifier's `rekor_max_tree_size`, and that any
    //! later bundle whose checkpoint reports a smaller tree_size is
    //! rejected as a rollback.

    use super::checkpoint::{assemble_bundle, build_bundle_pieces, mint_checkpoint};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use tako_governance::sigstore::KeylessVerifier;

    /// Two successful verifies on the same verifier instance with
    /// monotonically increasing checkpoint tree_size — both pass and
    /// the high-water mark advances to the larger value.
    #[test]
    fn monotonic_ascent_accepted_and_advances_high_water_mark() {
        let p = build_bundle_pieces();
        let root_b64 = B64.encode(p.true_root);

        let cp_small = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            5,
            &root_b64,
            "rekor.sigstore.dev",
        );
        let cp_big = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            7,
            &root_b64,
            "rekor.sigstore.dev",
        );
        let bundle_small = assemble_bundle(&p, cp_small);
        let bundle_big = assemble_bundle(&p, cp_big);

        let verifier = KeylessVerifier::new(p.identity.clone())
            .with_rekor_key(p.rekor_pem.as_bytes())
            .unwrap();

        assert_eq!(verifier.rekor_max_tree_size(), 0);
        verifier.verify_bundle(&p.manifest, &bundle_small).unwrap();
        assert_eq!(verifier.rekor_max_tree_size(), 5);
        verifier.verify_bundle(&p.manifest, &bundle_big).unwrap();
        assert_eq!(verifier.rekor_max_tree_size(), 7);
    }

    /// Verifying a bundle whose checkpoint reports a smaller tree_size
    /// than one already observed must be rejected with a clear
    /// rollback error message; the high-water mark must remain at the
    /// previously-observed value (i.e. the failed attempt does not
    /// regress it).
    #[test]
    fn rollback_rejected_after_higher_tree_size_observed() {
        let p = build_bundle_pieces();
        let root_b64 = B64.encode(p.true_root);

        let cp_big = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            10,
            &root_b64,
            "rekor.sigstore.dev",
        );
        let cp_small = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            5,
            &root_b64,
            "rekor.sigstore.dev",
        );
        let bundle_big = assemble_bundle(&p, cp_big);
        let bundle_small = assemble_bundle(&p, cp_small);

        let verifier = KeylessVerifier::new(p.identity.clone())
            .with_rekor_key(p.rekor_pem.as_bytes())
            .unwrap();

        verifier.verify_bundle(&p.manifest, &bundle_big).unwrap();
        assert_eq!(verifier.rekor_max_tree_size(), 10);

        let err = verifier
            .verify_bundle(&p.manifest, &bundle_small)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("tree_size 5 < previously observed 10"),
            "expected rollback error, got: {msg}"
        );
        // High-water mark unchanged.
        assert_eq!(verifier.rekor_max_tree_size(), 10);
    }

    /// A seeded verifier (constructed via `with_rekor_min_tree_size`)
    /// must reject bundles whose checkpoint tree_size is below the
    /// seed even on first observation — the seed is the persisted
    /// "trust root" for the freshness anchor.
    #[test]
    fn seed_value_enforced_from_construction() {
        let p = build_bundle_pieces();
        let root_b64 = B64.encode(p.true_root);

        let cp = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            5,
            &root_b64,
            "rekor.sigstore.dev",
        );
        let bundle_bytes = assemble_bundle(&p, cp);

        let verifier = KeylessVerifier::new(p.identity.clone())
            .with_rekor_key(p.rekor_pem.as_bytes())
            .unwrap()
            .with_rekor_min_tree_size(10);
        assert_eq!(verifier.rekor_max_tree_size(), 10);

        let err = verifier
            .verify_bundle(&p.manifest, &bundle_bytes)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("tree_size 5 < previously observed 10"),
            "expected seeded-rollback error, got: {msg}"
        );
        // Seed not regressed by the failed attempt.
        assert_eq!(verifier.rekor_max_tree_size(), 10);
    }
}

mod state_store_seed_persist {
    //! Phase 10.A — `JsonStateStore` round-trip against a real
    //! `KeylessVerifier`. The file-only unit tests for `JsonStateStore`
    //! (atomic write, missing-file load, parse error) live in
    //! `src/sigstore_state.rs::tests`; this module covers the
    //! seed → verify → persist cycle that needs the existing bundle
    //! fixture helpers.

    use super::checkpoint::{assemble_bundle, build_bundle_pieces, mint_checkpoint};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use tako_governance::sigstore::KeylessVerifier;
    use tako_governance::sigstore_state::JsonStateStore;

    /// Persist after a verify cycle: the on-disk value matches
    /// `verifier.rekor_max_tree_size()` and a fresh verifier seeded
    /// from the same store rejects a smaller-tree-size bundle.
    #[test]
    fn seed_then_verify_then_persist_round_trips_high_water_mark() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStateStore::new(dir.path().join("rekor.json"));

        let p = build_bundle_pieces();
        let root_b64 = B64.encode(p.true_root);
        let cp = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            8,
            &root_b64,
            "rekor.sigstore.dev",
        );
        let bundle = assemble_bundle(&p, cp);

        let verifier = store
            .seed(
                KeylessVerifier::new(p.identity.clone())
                    .with_rekor_key(p.rekor_pem.as_bytes())
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(verifier.rekor_max_tree_size(), 0); // first-boot seed

        verifier.verify_bundle(&p.manifest, &bundle).unwrap();
        assert_eq!(verifier.rekor_max_tree_size(), 8);

        store.persist(&verifier).unwrap();
        assert_eq!(store.load().unwrap(), 8);

        // Simulate process restart: a fresh verifier seeded from the
        // same store inherits the high-water mark and rejects a
        // smaller-tree-size bundle on first observation.
        let smaller_cp = mint_checkpoint(
            &p.rekor_signer,
            "rekor.sigstore.dev - test-fixture",
            5,
            &root_b64,
            "rekor.sigstore.dev",
        );
        let smaller = assemble_bundle(&p, smaller_cp);
        let restarted = store
            .seed(
                KeylessVerifier::new(p.identity.clone())
                    .with_rekor_key(p.rekor_pem.as_bytes())
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(restarted.rekor_max_tree_size(), 8);

        let err = restarted.verify_bundle(&p.manifest, &smaller).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("tree_size 5 < previously observed 8"),
            "expected restarted-rollback error, got: {msg}"
        );
    }
}
