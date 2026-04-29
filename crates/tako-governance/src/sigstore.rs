//! Sigstore-based verification of MCP tool catalogues.
//!
//! For supply-chain integrity, an operator can pin the exact set of tools
//! an MCP server is permitted to expose by:
//!
//! 1. Authoring a JSON [`Catalogue`] of allowed [`ToolSchema`]s.
//! 2. Signing it with `cosign sign-blob` (keyed or keyless).
//! 3. Distributing the catalogue + signature alongside the MCP server.
//! 4. Verifying both at startup before handing the schemas to
//!    `ToolRegistry::register_mcp`.
//!
//! Two trust models are supported:
//!
//! - [`CatalogueVerifier`] — **keyed** verification (Phase 4). A pinned
//!   public key (PEM, typically `cosign.pub`) signs every catalogue.
//!   Simple to deploy; rotation requires shipping a new key.
//! - [`KeylessVerifier`] — **keyless** verification (Phase 5). The
//!   catalogue is signed by a short-lived Fulcio-issued leaf certificate
//!   that binds the artifact to a specific OIDC identity (issuer + SAN).
//!   The operator pins an [`IdentityPolicy`] describing the expected
//!   identity; rotation is automatic because each signing operation
//!   gets a fresh leaf cert.
//!
//! ## v0.6.0 keyless scope
//!
//! The keyless verifier in v0.6.0 ships **leaf-cert + identity-policy +
//! signature** verification: it parses the leaf certificate from the
//! bundle, enforces the operator-supplied [`IdentityPolicy`] against the
//! cert's SAN / OIDC issuer extension, checks the cert's validity period
//! and Code Signing EKU, and verifies the signature using the cert's
//! public key. **Chain-of-trust validation against the Fulcio root** and
//! **Rekor inclusion-proof / SET verification** are intentionally
//! out of scope for v0.6.0 — operators are expected to validate those
//! pieces with `cosign verify-blob` at deploy time and ship a
//! pre-validated bundle. Both will lift into [`KeylessVerifier`]
//! transparently in a follow-up; the [`KeylessVerifier::verify_bundle`]
//! return shape won't change.
//!
//! Gated behind the `sigstore` Cargo feature so the heavy `sigstore`
//! crate (and its `aws-lc-rs` crypto backend) only land in the dep
//! tree when explicitly enabled.

use std::path::Path;

use ::sigstore::crypto::{CosignVerificationKey, Signature, SigningScheme};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use const_oid::ObjectIdentifier;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tako_core::{TakoError, ToolSchema};
use x509_cert::Certificate;
use x509_cert::der::{Decode, DecodePem, Encode, asn1::Ia5StringRef};
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{ExtendedKeyUsage, SubjectAltName};

/// A verified MCP tool catalogue.
///
/// Wire format (JSON):
///
/// ```json
/// {
///   "server": "optional human-readable identifier",
///   "tools": [
///     {
///       "name": "weather.lookup",
///       "description": "...",
///       "input_schema": { "type": "object", ... }
///     }
///   ]
/// }
/// ```
///
/// Round-trips through [`serde_json`]; `server` is optional and used
/// only for audit logging.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalogue {
    /// Free-form server identifier (e.g. `https://mcp.example.com`)
    /// included in audit records. Not enforced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Allowed tool schemas. Pass these straight to
    /// `tako_mcp::ToolRegistry::register_mcp`.
    pub tools: Vec<ToolSchema>,
}

/// Verifies cosign-signed MCP tool catalogues against a pinned public
/// key.
pub struct CatalogueVerifier {
    key: CosignVerificationKey,
}

impl std::fmt::Debug for CatalogueVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogueVerifier").finish_non_exhaustive()
    }
}

impl CatalogueVerifier {
    /// Load a verifier from PEM-encoded public-key bytes (typically the
    /// contents of a `cosign.pub` file).
    ///
    /// Defaults to `ECDSA_P256_SHA256_ASN.1` — the cosign default. To
    /// pin a different signing scheme (e.g. Ed25519), use
    /// [`Self::with_scheme`].
    pub fn from_pem(pem: &[u8]) -> Result<Self, TakoError> {
        Self::with_scheme(pem, &SigningScheme::default())
    }

    /// Load a verifier with an explicit signing scheme.
    pub fn with_scheme(pem: &[u8], scheme: &SigningScheme) -> Result<Self, TakoError> {
        let key = CosignVerificationKey::from_pem(pem, scheme)
            .map_err(|e| TakoError::Invalid(format!("sigstore: invalid public key: {e}")))?;
        Ok(Self { key })
    }

    /// Convenience: read the PEM from a filesystem path.
    pub fn from_pem_path(path: impl AsRef<Path>) -> Result<Self, TakoError> {
        let pem = std::fs::read(path.as_ref())
            .map_err(|e| TakoError::Invalid(format!("sigstore: read pem: {e}")))?;
        Self::from_pem(&pem)
    }

    /// Verify that `signature` is a valid cosign signature over
    /// `manifest_bytes` and return the parsed [`Catalogue`].
    ///
    /// Both raw and base64-encoded signatures are accepted —
    /// `cosign sign-blob` writes base64 by default; piping the output
    /// of `--output-signature` works directly.
    pub fn verify(&self, manifest_bytes: &[u8], signature: &[u8]) -> Result<Catalogue, TakoError> {
        let sig = if looks_base64(signature) {
            Signature::Base64Encoded(signature)
        } else {
            Signature::Raw(signature)
        };
        self.key
            .verify_signature(sig, manifest_bytes)
            .map_err(|e| TakoError::Invalid(format!("sigstore: signature invalid: {e}")))?;

        let catalogue: Catalogue = serde_json::from_slice(manifest_bytes)
            .map_err(|e| TakoError::Invalid(format!("sigstore: catalogue parse: {e}")))?;
        Ok(catalogue)
    }
}

/// Heuristic: cosign's signature output is base64 over a small alphabet.
/// Treat trailing whitespace as part of the encoding.
fn looks_base64(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.iter().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'\r' | b'\n' | b' ')
        })
}

// ---------------------------------------------------------------------------
// Keyless verification (Phase 5).
// ---------------------------------------------------------------------------

/// Fulcio v1 OIDC issuer extension. The value is an IA5String holding the
/// OIDC issuer URI (e.g. `https://accounts.google.com`).
const FULCIO_OIDC_ISSUER_V1: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1.1");
/// Fulcio v2 OIDC issuer extension. Value is a UTF8String wrapped in an
/// `OtherName` GeneralName; included for forward compatibility.
const FULCIO_OIDC_ISSUER_V2: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1.8");

/// Wire format for a tako keyless bundle.
///
/// This is a small JSON wrapper that an operator can produce from the
/// output of `cosign sign-blob` in a few lines of shell — see the
/// `tako.sigstore` module docs for a recipe. It deliberately decouples
/// `tako` from cosign's protobuf bundle format so we don't pull the
/// heavy `sigstore` `verify` feature (which transitively requires
/// `webbrowser`, `openidconnect`, etc.) into the dep tree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeylessBundle {
    /// PEM-encoded leaf certificate issued by Fulcio (or any CA whose
    /// trust the operator has externally validated).
    pub leaf_cert_pem: String,
    /// Base64-encoded signature over the manifest bytes. Matches what
    /// `cosign sign-blob` writes to `--output-signature`.
    pub signature_b64: String,
}

/// How to match the Subject Alternative Name on a Fulcio leaf cert.
#[derive(Clone, Debug)]
pub enum SanMatch {
    /// Exact string match against the SAN value (URI or rfc822 email).
    Exact(String),
    /// Anchored regex match against the SAN value.
    Regex(Regex),
}

impl SanMatch {
    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Exact(s) => s == value,
            Self::Regex(r) => r.is_match(value),
        }
    }
}

/// Identity binding the operator requires the leaf cert to satisfy.
///
/// Both fields are mandatory: a leaf cert must declare a matching OIDC
/// issuer **and** a matching SAN. This mirrors `cosign verify-blob
/// --certificate-identity ... --certificate-oidc-issuer ...`.
#[derive(Clone, Debug)]
pub struct IdentityPolicy {
    /// Expected OIDC issuer URI (e.g. `https://token.actions.githubusercontent.com`).
    pub issuer: String,
    /// Match for the SAN value embedded by Fulcio (typically a URI for
    /// machine identities or an email for human signers).
    pub san_match: SanMatch,
}

impl IdentityPolicy {
    /// Convenience: exact-match policy (issuer URL + SAN literal).
    pub fn exact(issuer: impl Into<String>, san: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            san_match: SanMatch::Exact(san.into()),
        }
    }

    /// Convenience: regex-match policy on the SAN.
    pub fn regex(issuer: impl Into<String>, san_pattern: &str) -> Result<Self, TakoError> {
        let r = Regex::new(san_pattern).map_err(|e| {
            TakoError::Invalid(format!("sigstore: invalid SAN regex `{san_pattern}`: {e}"))
        })?;
        Ok(Self {
            issuer: issuer.into(),
            san_match: SanMatch::Regex(r),
        })
    }
}

/// Verifies cosign keyless-style bundles where the leaf certificate
/// carries the signing identity.
pub struct KeylessVerifier {
    identity: IdentityPolicy,
}

impl std::fmt::Debug for KeylessVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeylessVerifier")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl KeylessVerifier {
    /// Build a verifier with the supplied identity policy.
    pub fn new(identity: IdentityPolicy) -> Self {
        Self { identity }
    }

    /// Verify a keyless bundle and return the parsed [`Catalogue`].
    ///
    /// Steps performed, in order:
    ///
    /// 1. Parse the bundle JSON (`bundle_json`).
    /// 2. Parse the leaf cert PEM.
    /// 3. Check `not_before <= now <= not_after`.
    /// 4. Confirm the cert carries the Code Signing extended key usage
    ///    (Fulcio always sets this).
    /// 5. Extract the OIDC issuer extension and the SAN.
    /// 6. Match both against the [`IdentityPolicy`].
    /// 7. Verify the base64-decoded signature against `manifest_bytes`
    ///    using the leaf cert's public key.
    /// 8. Deserialize `manifest_bytes` as a [`Catalogue`].
    pub fn verify_bundle(
        &self,
        manifest_bytes: &[u8],
        bundle_json: &[u8],
    ) -> Result<Catalogue, TakoError> {
        let bundle: KeylessBundle = serde_json::from_slice(bundle_json)
            .map_err(|e| TakoError::Invalid(format!("sigstore: bundle parse: {e}")))?;
        let cert = parse_leaf_cert(bundle.leaf_cert_pem.as_bytes())?;
        check_validity_now(&cert)?;
        check_code_signing_eku(&cert)?;

        let san = extract_san_value(&cert)?;
        let issuer = extract_oidc_issuer(&cert)?;
        if issuer != self.identity.issuer {
            return Err(TakoError::Invalid(format!(
                "sigstore: cert OIDC issuer `{issuer}` does not match expected `{}`",
                self.identity.issuer
            )));
        }
        if !self.identity.san_match.matches(&san) {
            return Err(TakoError::Invalid(format!(
                "sigstore: cert SAN `{san}` does not match identity policy"
            )));
        }

        let signature_bytes = B64
            .decode(bundle.signature_b64.trim())
            .map_err(|e| TakoError::Invalid(format!("sigstore: signature base64: {e}")))?;
        let scheme = signing_scheme_for_cert(&cert)?;
        let spki_der = cert
            .tbs_certificate
            .subject_public_key_info
            .to_der()
            .map_err(|e| TakoError::Invalid(format!("sigstore: spki encode: {e}")))?;
        let key = CosignVerificationKey::from_der(&spki_der, &scheme).map_err(|e| {
            TakoError::Invalid(format!("sigstore: cert public key unsupported: {e}"))
        })?;
        key.verify_signature(Signature::Raw(&signature_bytes), manifest_bytes)
            .map_err(|e| TakoError::Invalid(format!("sigstore: signature invalid: {e}")))?;

        let catalogue: Catalogue = serde_json::from_slice(manifest_bytes)
            .map_err(|e| TakoError::Invalid(format!("sigstore: catalogue parse: {e}")))?;
        Ok(catalogue)
    }
}

fn parse_leaf_cert(pem_bytes: &[u8]) -> Result<Certificate, TakoError> {
    let pem_str = std::str::from_utf8(pem_bytes)
        .map_err(|e| TakoError::Invalid(format!("sigstore: leaf cert utf8: {e}")))?;
    Certificate::from_pem(pem_str)
        .map_err(|e| TakoError::Invalid(format!("sigstore: leaf cert parse: {e}")))
}

fn check_validity_now(cert: &Certificate) -> Result<(), TakoError> {
    let now = std::time::SystemTime::now();
    let validity = &cert.tbs_certificate.validity;
    if now < validity.not_before.to_system_time() {
        return Err(TakoError::Invalid(format!(
            "sigstore: cert not yet valid (notBefore={})",
            validity.not_before
        )));
    }
    if now > validity.not_after.to_system_time() {
        return Err(TakoError::Invalid(format!(
            "sigstore: cert expired (notAfter={})",
            validity.not_after
        )));
    }
    Ok(())
}

fn check_code_signing_eku(cert: &Certificate) -> Result<(), TakoError> {
    let eku_pair = cert
        .tbs_certificate
        .get::<ExtendedKeyUsage>()
        .map_err(|e| TakoError::Invalid(format!("sigstore: cert EKU extension: {e}")))?;
    let Some((_, eku)) = eku_pair else {
        return Err(TakoError::Invalid(
            "sigstore: cert missing ExtendedKeyUsage extension".into(),
        ));
    };
    // OID 1.3.6.1.5.5.7.3.3 — id-kp-codeSigning
    let code_signing: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.3");
    if !eku.0.contains(&code_signing) {
        return Err(TakoError::Invalid(
            "sigstore: cert EKU does not include codeSigning".into(),
        ));
    }
    Ok(())
}

fn extract_san_value(cert: &Certificate) -> Result<String, TakoError> {
    let san_pair = cert
        .tbs_certificate
        .get::<SubjectAltName>()
        .map_err(|e| TakoError::Invalid(format!("sigstore: cert SAN extension: {e}")))?;
    let Some((_, san)) = san_pair else {
        return Err(TakoError::Invalid(
            "sigstore: cert missing SubjectAltName extension".into(),
        ));
    };
    for name in &san.0 {
        match name {
            GeneralName::Rfc822Name(s) => return Ok(s.as_str().to_string()),
            GeneralName::UniformResourceIdentifier(s) => return Ok(s.as_str().to_string()),
            GeneralName::DnsName(s) => return Ok(s.as_str().to_string()),
            _ => continue,
        }
    }
    Err(TakoError::Invalid(
        "sigstore: cert SAN has no rfc822/URI/DNS entry".into(),
    ))
}

fn extract_oidc_issuer(cert: &Certificate) -> Result<String, TakoError> {
    let extensions = cert
        .tbs_certificate
        .extensions
        .as_ref()
        .ok_or_else(|| TakoError::Invalid("sigstore: cert has no extensions".into()))?;
    for ext in extensions {
        if ext.extn_id == FULCIO_OIDC_ISSUER_V1 {
            // v1 stores the issuer URI as a raw IA5String *without* the
            // surrounding ASN.1 tag — i.e. the extn_value is the bytes
            // of the URI directly.
            let s = std::str::from_utf8(ext.extn_value.as_bytes())
                .map_err(|e| TakoError::Invalid(format!("sigstore: OIDC issuer (v1) utf8: {e}")))?;
            return Ok(s.to_string());
        }
        if ext.extn_id == FULCIO_OIDC_ISSUER_V2 {
            // v2 wraps the URI in a DER UTF8String; decode it.
            let s: Ia5StringRef<'_> = Ia5StringRef::from_der(ext.extn_value.as_bytes())
                .map_err(|e| TakoError::Invalid(format!("sigstore: OIDC issuer (v2): {e}")))?;
            return Ok(s.as_str().to_string());
        }
    }
    Err(TakoError::Invalid(
        "sigstore: cert has no Fulcio OIDC issuer extension (v1 or v2)".into(),
    ))
}

fn signing_scheme_for_cert(cert: &Certificate) -> Result<SigningScheme, TakoError> {
    let alg_oid = &cert.tbs_certificate.subject_public_key_info.algorithm.oid;
    // ECDSA with P-256: the SPKI alg is id-ecPublicKey 1.2.840.10045.2.1
    // and the parameters carry the named curve OID. Fulcio always emits
    // P-256 today; we accept that as the default and match the
    // signature_algorithm OID to choose the digest.
    let sig_alg = &cert.signature_algorithm.oid;
    // 1.2.840.10045.4.3.2  — ecdsa-with-SHA256
    // 1.2.840.10045.4.3.3  — ecdsa-with-SHA384
    // 1.2.840.113549.1.1.11 — sha256WithRSAEncryption
    let ecdsa_p256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
    let ecdsa_p384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
    let rsa_sha256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
    let ec_pubkey: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
    let rsa_pubkey: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");

    if sig_alg == &ecdsa_p256 && alg_oid == &ec_pubkey {
        return Ok(SigningScheme::ECDSA_P256_SHA256_ASN1);
    }
    if sig_alg == &ecdsa_p384 && alg_oid == &ec_pubkey {
        return Ok(SigningScheme::ECDSA_P384_SHA384_ASN1);
    }
    if sig_alg == &rsa_sha256 && alg_oid == &rsa_pubkey {
        return Ok(SigningScheme::RSA_PKCS1_SHA256(2048));
    }
    Err(TakoError::Invalid(format!(
        "sigstore: unsupported cert signature algorithm `{sig_alg}` over key `{alg_oid}`"
    )))
}
