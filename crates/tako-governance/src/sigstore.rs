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
    /// Optional concatenated PEM block carrying the intermediate
    /// certificate(s) between the leaf and a Fulcio root. When the
    /// verifier has been configured with a [`TrustRoot`], every cert in
    /// this chain is signature-validated; the leaf must terminate at one
    /// of the trust root's roots. When no trust root is configured this
    /// field is ignored (back-compat with v0.6.0 bundles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_pem: Option<String>,
    /// Optional Rekor transparency-log entry. When the verifier has been
    /// configured with a Rekor public key (see
    /// [`KeylessVerifier::with_rekor_key`]) and this field is present,
    /// the SET (Signed Entry Timestamp) is verified against the pinned
    /// key. If the entry also carries an
    /// [`RekorInclusionProof`](RekorEntry::inclusion_proof) (added in
    /// v0.8.0), the Merkle audit path is verified against the supplied
    /// tree root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rekor: Option<RekorEntry>,
}

/// Rekor transparency-log entry as carried in a tako keyless bundle.
///
/// Field semantics follow the Rekor `LogEntry` v0.0.1 schema:
/// <https://github.com/sigstore/rekor>. The SET is an ECDSA-P256
/// signature by Rekor's public key over a canonical JSON of
/// `{body, integratedTime, logID, logIndex}` (sorted keys, no
/// whitespace).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RekorEntry {
    /// Rekor log index (monotonic counter).
    pub log_index: u64,
    /// Hex SHA-256 of the Rekor public key the SET was issued under.
    pub log_id: String,
    /// Unix-seconds time Rekor integrated the entry.
    pub integrated_time: i64,
    /// Base64-encoded canonicalised body of the Rekor entry. The body
    /// is itself a JSON document carrying the manifest digest and the
    /// leaf certificate.
    pub canonicalized_body: String,
    /// Base64-encoded SET (ECDSA-P256 signature by Rekor's public key
    /// over the canonical entry JSON).
    pub set_b64: String,
    /// Optional Merkle inclusion proof against the Rekor tree head.
    /// When present and the verifier has been configured with a Rekor
    /// key (see [`KeylessVerifier::with_rekor_key`]), the audit path is
    /// verified per RFC 6962 §2.1.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inclusion_proof: Option<RekorInclusionProof>,
    /// Optional Rekor checkpoint (`SignedNote` over the tree head).
    /// When present and a Rekor key is pinned, the checkpoint
    /// signature is verified against that key. If the entry also
    /// carries an `inclusion_proof`, the checkpoint's `root_hash` must
    /// agree with the inclusion proof's `root_hash_hex` — this anchors
    /// the per-entry audit path to a tree head the operator can also
    /// observe out-of-band. Added in v0.9.0; serde-default `None` so
    /// pre-v0.9.0 bundles deserialize unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<RekorCheckpoint>,
}

/// Rekor checkpoint (`SignedNote`) over the transparency-log tree head.
///
/// The checkpoint is a small text artefact of the form:
///
/// ```text
/// <origin>\n<tree_size>\n<base64 root_hash>\n
/// \n
/// — <key_id> <base64 signature>\n
/// ```
///
/// where the signed message is the first three lines plus the empty
/// separator (i.e. `format!("{origin}\n{tree_size}\n{root_hash_b64}\n\n")`).
/// The signature is produced by the same Rekor public-good ECDSA-P256
/// key already used to sign per-entry SETs.
///
/// See <https://github.com/transparency-dev/formats/blob/main/log/README.md>
/// for the upstream `SignedNote` format.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RekorCheckpoint {
    /// Free-form origin / log name (first line of the note body).
    /// Must match the origin the Rekor instance writes (e.g.
    /// `"rekor.sigstore.dev - 1193050959916656506"`).
    pub origin: String,
    /// Tree size at the moment the checkpoint was issued. Must match
    /// `RekorInclusionProof::tree_size` when both are present.
    pub tree_size: u64,
    /// Base64-encoded SHA-256 root hash of the Rekor tree at
    /// `tree_size`. Must round-trip to the same bytes as
    /// `RekorInclusionProof::root_hash_hex` (modulo encoding) when
    /// both are present.
    pub root_hash_b64: String,
    /// Short-form key identifier from the trailing `— <key_id>` line.
    /// Carried verbatim into the audit log; not used for verification
    /// (the operator pins the full Rekor key separately).
    pub key_id: String,
    /// Base64-encoded ECDSA-P256 signature over the canonical signed
    /// message described in the type-level docs.
    pub signature_b64: String,
}

/// Merkle inclusion proof against a Rekor tree head.
///
/// Field semantics follow the Rekor `LogEntry.verification.inclusionProof`
/// JSON shape: <https://github.com/sigstore/rekor>. Hashes are
/// hex-encoded SHA-256 digests; the proof is verified per RFC 6962
/// §2.1.1 with leaf hash `SHA256(0x00 || canonicalized_body)` and
/// internal-node hash `SHA256(0x01 || left || right)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RekorInclusionProof {
    /// Sibling hash list (hex), bottom-up, from the leaf's neighbour
    /// to the children of the root.
    pub hashes_hex: Vec<String>,
    /// Total number of leaves in the Rekor tree at the moment the
    /// proof was issued.
    pub tree_size: u64,
    /// 0-based index of the entry's leaf within the tree.
    pub log_index: u64,
    /// Hex-encoded root hash of the Rekor tree at `tree_size`.
    pub root_hash_hex: String,
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

/// Operator-pinned trust anchors for chain-of-trust validation.
///
/// Both fields hold zero or more PEM-loaded X.509 certificates. The
/// [`KeylessVerifier`] walks the bundle's chain from the leaf upward,
/// preferring an `intermediates`-resident issuer at each step and
/// terminating at one of the `roots`. Certificates beyond the trust
/// root are rejected.
///
/// Operators typically populate `roots` from the Sigstore public-good
/// trust-root (`fulcio.crt.pem`); private deployments substitute their
/// own internal CA. `intermediates` covers Fulcio's intermediate CA(s)
/// and is optional only because Fulcio currently issues from a root
/// directly in some deployments.
#[derive(Clone, Default)]
pub struct TrustRoot {
    roots: Vec<Certificate>,
    intermediates: Vec<Certificate>,
}

impl std::fmt::Debug for TrustRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustRoot")
            .field("roots", &self.roots.len())
            .field("intermediates", &self.intermediates.len())
            .finish()
    }
}

impl TrustRoot {
    /// Build a trust root from concatenated PEM blocks.
    ///
    /// `roots_pem` and `intermediates_pem` may each contain one or more
    /// `BEGIN CERTIFICATE` blocks; the bytes are split on the PEM
    /// boundaries and parsed individually. Pass `None` for
    /// `intermediates_pem` when the Fulcio deployment issues straight
    /// from a root.
    pub fn from_pem(roots_pem: &[u8], intermediates_pem: Option<&[u8]>) -> Result<Self, TakoError> {
        let roots = parse_pem_chain(roots_pem)?;
        if roots.is_empty() {
            return Err(TakoError::Invalid(
                "sigstore: trust root has no root certificates".into(),
            ));
        }
        let intermediates = match intermediates_pem {
            Some(b) => parse_pem_chain(b)?,
            None => Vec::new(),
        };
        Ok(Self {
            roots,
            intermediates,
        })
    }

    /// Convenience: read both files from the filesystem.
    pub fn from_paths(
        roots_path: impl AsRef<Path>,
        intermediates_path: Option<impl AsRef<Path>>,
    ) -> Result<Self, TakoError> {
        let roots_bytes = std::fs::read(roots_path.as_ref())
            .map_err(|e| TakoError::Invalid(format!("sigstore: read roots pem: {e}")))?;
        let intermediates_bytes =
            match intermediates_path {
                Some(p) => Some(std::fs::read(p.as_ref()).map_err(|e| {
                    TakoError::Invalid(format!("sigstore: read intermediates: {e}"))
                })?),
                None => None,
            };
        Self::from_pem(&roots_bytes, intermediates_bytes.as_deref())
    }
}

/// Verifies cosign keyless-style bundles where the leaf certificate
/// carries the signing identity.
pub struct KeylessVerifier {
    identity: IdentityPolicy,
    trust_root: Option<TrustRoot>,
    rekor_key: Option<CosignVerificationKey>,
}

impl std::fmt::Debug for KeylessVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeylessVerifier")
            .field("identity", &self.identity)
            .field("trust_root", &self.trust_root)
            .field("rekor_key_pinned", &self.rekor_key.is_some())
            .finish_non_exhaustive()
    }
}

impl KeylessVerifier {
    /// Build a verifier with the supplied identity policy.
    pub fn new(identity: IdentityPolicy) -> Self {
        Self {
            identity,
            trust_root: None,
            rekor_key: None,
        }
    }

    /// Pin a [`TrustRoot`] so chain-of-trust validation runs at
    /// verification time. Each cert in the bundle's `chain_pem` plus the
    /// leaf are walked toward the root; expired, unknown-issuer, or
    /// signature-mismatched certs raise. When no trust root is set
    /// (the v0.6.0 default), chain validation is skipped — operators
    /// are expected to pre-validate with `cosign verify-blob`.
    pub fn with_trust_root(mut self, root: TrustRoot) -> Self {
        self.trust_root = Some(root);
        self
    }

    /// Pin a Rekor public key (PEM, ECDSA-P256). When set and the bundle
    /// carries a [`RekorEntry`], the SET is verified against the key.
    /// Without this, Rekor verification is skipped even if the bundle
    /// contains a `rekor` field.
    pub fn with_rekor_key(mut self, pem: &[u8]) -> Result<Self, TakoError> {
        let key = CosignVerificationKey::from_pem(pem, &SigningScheme::ECDSA_P256_SHA256_ASN1)
            .map_err(|e| TakoError::Invalid(format!("sigstore: rekor key: {e}")))?;
        self.rekor_key = Some(key);
        Ok(self)
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

        // 6.D: chain-of-trust walk when a TrustRoot is pinned.
        if let Some(root) = self.trust_root.as_ref() {
            let intermediates = match bundle.chain_pem.as_deref() {
                Some(s) => parse_pem_chain(s.as_bytes())?,
                None => Vec::new(),
            };
            verify_chain(&cert, &intermediates, root)?;
        }

        // 6.E: Rekor SET verification when both a Rekor key is pinned
        // and the bundle carries a transparency-log entry.
        if let (Some(rekor_key), Some(entry)) = (self.rekor_key.as_ref(), bundle.rekor.as_ref()) {
            verify_rekor_set(rekor_key, entry)?;
            // 7.A: Rekor Merkle inclusion-proof verification, when the
            // entry also carries one. SET binds the entry's metadata to
            // a moment in time; the inclusion proof binds it to a
            // specific position in the public log.
            if let Some(proof) = entry.inclusion_proof.as_ref() {
                verify_rekor_inclusion(entry, proof)?;
            }
            // 8.C: Rekor checkpoint (SignedNote) verification when the
            // entry also carries one. The checkpoint anchors the tree
            // head out-of-band — independent of the per-entry SET —
            // and (when an inclusion proof is also present) must agree
            // with the inclusion proof's pinned root hash.
            if let Some(checkpoint) = entry.checkpoint.as_ref() {
                let expected_root_hex = entry
                    .inclusion_proof
                    .as_ref()
                    .map(|p| p.root_hash_hex.as_str());
                verify_rekor_checkpoint(rekor_key, checkpoint, expected_root_hex)?;
            }
        }

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

// ---------------------------------------------------------------------------
// Chain-of-trust helpers (Phase 6).
// ---------------------------------------------------------------------------

/// Split a concatenated PEM document into individual `Certificate`s.
fn parse_pem_chain(pem_bytes: &[u8]) -> Result<Vec<Certificate>, TakoError> {
    let pem_str = std::str::from_utf8(pem_bytes)
        .map_err(|e| TakoError::Invalid(format!("sigstore: chain pem utf8: {e}")))?;
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_block = false;
    for line in pem_str.lines() {
        if line.contains("-----BEGIN CERTIFICATE-----") {
            in_block = true;
            buf.clear();
            buf.push_str(line);
            buf.push('\n');
        } else if line.contains("-----END CERTIFICATE-----") {
            buf.push_str(line);
            buf.push('\n');
            let cert = Certificate::from_pem(&buf)
                .map_err(|e| TakoError::Invalid(format!("sigstore: chain cert parse: {e}")))?;
            out.push(cert);
            buf.clear();
            in_block = false;
        } else if in_block {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    Ok(out)
}

/// Determine the [`SigningScheme`] for verifying `child.signature` using
/// `issuer`'s public key. The signature algorithm comes from `child`'s
/// `signature_algorithm` field; the SPKI key alg must match it.
fn chain_signing_scheme(
    child: &Certificate,
    issuer: &Certificate,
) -> Result<SigningScheme, TakoError> {
    let sig_alg = &child.signature_algorithm.oid;
    let key_alg = &issuer.tbs_certificate.subject_public_key_info.algorithm.oid;
    let ecdsa_p256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
    let ecdsa_p384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
    let rsa_sha256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
    let ec_pubkey: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
    let rsa_pubkey: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");

    if sig_alg == &ecdsa_p256 && key_alg == &ec_pubkey {
        return Ok(SigningScheme::ECDSA_P256_SHA256_ASN1);
    }
    if sig_alg == &ecdsa_p384 && key_alg == &ec_pubkey {
        return Ok(SigningScheme::ECDSA_P384_SHA384_ASN1);
    }
    if sig_alg == &rsa_sha256 && key_alg == &rsa_pubkey {
        return Ok(SigningScheme::RSA_PKCS1_SHA256(2048));
    }
    Err(TakoError::Invalid(format!(
        "sigstore: unsupported chain signature algorithm `{sig_alg}` over key `{key_alg}`"
    )))
}

/// Walk leaf → intermediates → trust root, verifying every signature.
///
/// Picks an issuer at each step by `subject == issuer` matching, first
/// in `bundle_intermediates` then in the pinned trust-root's
/// `intermediates`, finally in `roots`. A self-signed cert (subject ==
/// issuer) is treated as a root anchor and accepted only if it appears
/// in `trust_root.roots`.
fn verify_chain(
    leaf: &Certificate,
    bundle_intermediates: &[Certificate],
    trust_root: &TrustRoot,
) -> Result<(), TakoError> {
    let mut current = leaf;
    for hop in 0..16 {
        let _ = hop;
        check_validity_now(current)?;
        let issuer_subject = &current.tbs_certificate.issuer;

        // Self-signed: require it to appear in trust_root.roots.
        if &current.tbs_certificate.subject == issuer_subject {
            if trust_root.roots.iter().any(|r| cert_eq(r, current)) {
                // Verify the self-signature against itself as a sanity
                // check; root certs are signed by their own private key.
                verify_one_signature(current, current)?;
                return Ok(());
            }
            return Err(TakoError::Invalid(
                "sigstore: chain ends at a self-signed cert that is not in the pinned trust root"
                    .into(),
            ));
        }

        let issuer = bundle_intermediates
            .iter()
            .find(|c| &c.tbs_certificate.subject == issuer_subject)
            .or_else(|| {
                trust_root
                    .intermediates
                    .iter()
                    .find(|c| &c.tbs_certificate.subject == issuer_subject)
            })
            .or_else(|| {
                trust_root
                    .roots
                    .iter()
                    .find(|c| &c.tbs_certificate.subject == issuer_subject)
            })
            .ok_or_else(|| {
                TakoError::Invalid(
                    "sigstore: chain has unknown issuer (no matching intermediate or root)".into(),
                )
            })?;

        verify_one_signature(current, issuer)?;

        // If the issuer is a pinned root, we're done.
        if trust_root.roots.iter().any(|r| cert_eq(r, issuer)) {
            check_validity_now(issuer)?;
            return Ok(());
        }
        current = issuer;
    }
    Err(TakoError::Invalid(
        "sigstore: chain depth exceeded 16 hops without reaching a pinned root".into(),
    ))
}

/// Verify `child.signature` over `child.tbs_certificate` using
/// `issuer`'s public key.
fn verify_one_signature(child: &Certificate, issuer: &Certificate) -> Result<(), TakoError> {
    let scheme = chain_signing_scheme(child, issuer)?;
    let issuer_spki_der = issuer
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .map_err(|e| TakoError::Invalid(format!("sigstore: issuer spki encode: {e}")))?;
    let key = CosignVerificationKey::from_der(&issuer_spki_der, &scheme)
        .map_err(|e| TakoError::Invalid(format!("sigstore: issuer key unsupported: {e}")))?;
    let tbs_der = child
        .tbs_certificate
        .to_der()
        .map_err(|e| TakoError::Invalid(format!("sigstore: tbs encode: {e}")))?;
    let sig_bytes = child.signature.raw_bytes();
    key.verify_signature(Signature::Raw(sig_bytes), &tbs_der)
        .map_err(|e| TakoError::Invalid(format!("sigstore: chain signature invalid: {e}")))?;
    Ok(())
}

/// Compare two parsed certs by their DER serialisation.
fn cert_eq(a: &Certificate, b: &Certificate) -> bool {
    match (a.to_der(), b.to_der()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Rekor SET verification (Phase 6).
// ---------------------------------------------------------------------------

/// Verify a Rekor Signed Entry Timestamp (SET) against the pinned key.
///
/// The canonical SET payload is a JSON object with **sorted keys** and
/// no whitespace, of shape:
/// `{"body":"<base64>","integratedTime":<int>,"logID":"<hex>","logIndex":<int>}`.
/// See <https://github.com/sigstore/rekor/blob/main/pkg/types/intoto/v0.0.1/intoto_v0_0_1_schema.json>.
fn verify_rekor_set(key: &CosignVerificationKey, entry: &RekorEntry) -> Result<(), TakoError> {
    let canonical = format!(
        "{{\"body\":\"{body}\",\"integratedTime\":{ts},\"logID\":\"{log_id}\",\"logIndex\":{idx}}}",
        body = entry.canonicalized_body,
        ts = entry.integrated_time,
        log_id = entry.log_id,
        idx = entry.log_index,
    );
    let set_bytes = B64
        .decode(entry.set_b64.trim())
        .map_err(|e| TakoError::Invalid(format!("sigstore: rekor SET base64: {e}")))?;
    key.verify_signature(Signature::Raw(&set_bytes), canonical.as_bytes())
        .map_err(|e| TakoError::Invalid(format!("sigstore: rekor SET invalid: {e}")))?;
    Ok(())
}

/// Verify a Rekor checkpoint (`SignedNote`) signature against the
/// pinned Rekor key, and (when an `expected_root_hex` is supplied
/// alongside an inclusion proof) assert that the checkpoint's
/// `root_hash_b64` decodes to the same bytes as `expected_root_hex`.
///
/// The signed message is reconstructed as
/// `format!("{origin}\n{tree_size}\n{root_hash_b64}\n\n")` per the
/// `SignedNote` text format documented at
/// <https://github.com/transparency-dev/formats/blob/main/log/README.md>.
fn verify_rekor_checkpoint(
    key: &CosignVerificationKey,
    checkpoint: &RekorCheckpoint,
    expected_root_hex: Option<&str>,
) -> Result<(), TakoError> {
    let signed_message = format!(
        "{origin}\n{tree_size}\n{root_hash_b64}\n\n",
        origin = checkpoint.origin,
        tree_size = checkpoint.tree_size,
        root_hash_b64 = checkpoint.root_hash_b64,
    );
    let sig_bytes = B64
        .decode(checkpoint.signature_b64.trim())
        .map_err(|e| TakoError::Invalid(format!("sigstore: rekor checkpoint base64: {e}")))?;
    key.verify_signature(Signature::Raw(&sig_bytes), signed_message.as_bytes())
        .map_err(|e| TakoError::Invalid(format!("sigstore: rekor checkpoint invalid: {e}")))?;

    if let Some(expected_hex) = expected_root_hex {
        let actual_bytes = B64.decode(checkpoint.root_hash_b64.trim()).map_err(|e| {
            TakoError::Invalid(format!("sigstore: rekor checkpoint root base64: {e}"))
        })?;
        let expected_bytes = hex::decode(expected_hex.trim())
            .map_err(|e| TakoError::Invalid(format!("sigstore: rekor checkpoint root hex: {e}")))?;
        if actual_bytes != expected_bytes {
            return Err(TakoError::Invalid(
                "sigstore: rekor checkpoint root hash disagrees with inclusion proof root".into(),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// cosign protobuf-bundle adapter (Phase 7.C).
// ---------------------------------------------------------------------------

#[cfg(feature = "sigstore-protobuf")]
impl KeylessBundle {
    /// Decode a `cosign sign-blob --bundle out.pb` payload into the
    /// JSON-shaped [`KeylessBundle`] tako carries internally.
    ///
    /// Translates field-by-field:
    ///
    /// - `verification_material.x509_certificate_chain.certificates[0]`
    ///   (or, on newer cosign builds, `.certificate`) → `leaf_cert_pem`
    /// - any remaining chain certs → `chain_pem` (concatenated PEM)
    /// - `message_signature.signature` → base64 → `signature_b64`
    /// - first `verification_material.tlog_entries[]` → `Some(rekor)`
    ///   with `inclusion_promise.signed_entry_timestamp` →
    ///   `set_b64`, plus the `inclusion_proof` (when present)
    ///   translated into a [`RekorInclusionProof`].
    ///
    /// Available when the `sigstore-protobuf` feature is enabled.
    pub fn from_protobuf_bundle(bytes: &[u8]) -> Result<Self, TakoError> {
        use crate::cosign_bundle::{Bundle, X509Certificate};

        let bundle: Bundle = crate::cosign_bundle::decode(bytes)
            .map_err(|e| TakoError::Invalid(format!("sigstore: protobuf bundle decode: {e}")))?;

        let vm = bundle.verification_material.ok_or_else(|| {
            TakoError::Invalid("sigstore: protobuf bundle missing verification_material".into())
        })?;

        // Resolve the cert chain. Newer cosign builds emit a single
        // `certificate` (cert-of-record); older builds emit a chain.
        // Either form maps onto `(leaf, [intermediates...])`.
        let certs: Vec<X509Certificate> = match (vm.x509_certificate_chain, vm.certificate) {
            (Some(chain), _) if !chain.certificates.is_empty() => chain.certificates,
            (_, Some(single)) => vec![single],
            _ => {
                return Err(TakoError::Invalid(
                    "sigstore: protobuf bundle has no x509 certificate material".into(),
                ));
            }
        };
        let leaf_cert_pem = der_to_pem(&certs[0].raw_bytes)?;
        let chain_pem = if certs.len() > 1 {
            let mut out = String::new();
            for c in &certs[1..] {
                out.push_str(&der_to_pem(&c.raw_bytes)?);
            }
            Some(out)
        } else {
            None
        };

        let sig = bundle.message_signature.ok_or_else(|| {
            TakoError::Invalid(
                "sigstore: protobuf bundle missing message_signature (DSSE bundles unsupported)"
                    .into(),
            )
        })?;
        if sig.signature.is_empty() {
            return Err(TakoError::Invalid(
                "sigstore: protobuf bundle message_signature.signature is empty".into(),
            ));
        }
        let signature_b64 = B64.encode(&sig.signature);

        // Pick the first tlog entry, if any. In practice cosign emits
        // one Rekor entry per signing operation.
        let rekor = match vm.tlog_entries.into_iter().next() {
            Some(t) => {
                let log_id_bytes = t
                    .log_id
                    .ok_or_else(|| {
                        TakoError::Invalid(
                            "sigstore: protobuf bundle tlog entry missing log_id".into(),
                        )
                    })?
                    .key_id;
                let promise = t.inclusion_promise.ok_or_else(|| {
                    TakoError::Invalid(
                        "sigstore: protobuf bundle tlog entry missing inclusion_promise (SET)"
                            .into(),
                    )
                })?;
                let inclusion_proof = t.inclusion_proof.map(|p| RekorInclusionProof {
                    log_index: p.log_index as u64,
                    tree_size: p.tree_size as u64,
                    root_hash_hex: hex::encode(&p.root_hash),
                    hashes_hex: p.hashes.iter().map(hex::encode).collect(),
                });
                Some(RekorEntry {
                    log_index: t.log_index as u64,
                    log_id: hex::encode(&log_id_bytes),
                    integrated_time: t.integrated_time,
                    canonicalized_body: B64.encode(&t.canonicalized_body),
                    set_b64: B64.encode(&promise.signed_entry_timestamp),
                    inclusion_proof,
                    // Cosign protobuf bundles do not (currently) carry
                    // a SignedNote checkpoint inline; operators that
                    // want checkpoint verification must populate the
                    // field on the JSON-shaped KeylessBundle directly.
                    checkpoint: None,
                })
            }
            None => None,
        };

        Ok(KeylessBundle {
            leaf_cert_pem,
            signature_b64,
            chain_pem,
            rekor,
        })
    }
}

#[cfg(feature = "sigstore-protobuf")]
fn der_to_pem(der: &[u8]) -> Result<String, TakoError> {
    if der.is_empty() {
        return Err(TakoError::Invalid(
            "sigstore: protobuf bundle cert raw_bytes is empty".into(),
        ));
    }
    let p = pem::Pem::new("CERTIFICATE", der.to_vec());
    Ok(pem::encode(&p))
}

#[cfg(all(test, feature = "sigstore-protobuf"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod protobuf_tests {
    use super::*;
    use crate::cosign_bundle::{
        Bundle, InclusionPromise, InclusionProof, LogId, MessageSignature, TransparencyLogEntry,
        VerificationMaterial, X509Certificate, X509CertificateChain,
    };
    use prost::Message as _;

    /// A short DER blob that round-trips as cert-shaped bytes for the
    /// adapter; it is **not** a valid X.509 cert (the adapter does not
    /// parse the DER, just wraps it as PEM).
    fn dummy_der(seed: u8) -> Vec<u8> {
        vec![seed; 64]
    }

    fn sample_bundle() -> Bundle {
        Bundle {
            media_type: "application/vnd.dev.sigstore.bundle+json;version=0.2".into(),
            verification_material: Some(VerificationMaterial {
                x509_certificate_chain: Some(X509CertificateChain {
                    certificates: vec![
                        X509Certificate {
                            raw_bytes: dummy_der(0xAA),
                        },
                        X509Certificate {
                            raw_bytes: dummy_der(0xBB),
                        },
                    ],
                }),
                certificate: None,
                tlog_entries: vec![TransparencyLogEntry {
                    log_index: 7777,
                    log_id: Some(LogId {
                        key_id: vec![1, 2, 3, 4],
                    }),
                    integrated_time: 1_700_000_000,
                    inclusion_promise: Some(InclusionPromise {
                        signed_entry_timestamp: vec![0xDE, 0xAD, 0xBE, 0xEF],
                    }),
                    inclusion_proof: Some(InclusionProof {
                        log_index: 7777,
                        root_hash: vec![0x12; 32],
                        tree_size: 12345,
                        hashes: vec![vec![0x34; 32], vec![0x56; 32]],
                    }),
                    canonicalized_body: b"rekor-body-bytes".to_vec(),
                }],
            }),
            message_signature: Some(MessageSignature {
                message_digest: None,
                signature: vec![0x99, 0x88, 0x77, 0x66],
            }),
        }
    }

    #[test]
    fn round_trips_protobuf_bundle_into_keyless_bundle() {
        let pb = sample_bundle();
        let bytes = pb.encode_to_vec();

        let kb = KeylessBundle::from_protobuf_bundle(&bytes).unwrap();
        assert!(kb.leaf_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(kb.leaf_cert_pem.contains("END CERTIFICATE"));
        // Chain PEM carries the second cert.
        assert!(kb.chain_pem.unwrap().contains("BEGIN CERTIFICATE"));
        assert_eq!(
            B64.decode(&kb.signature_b64).unwrap(),
            vec![0x99, 0x88, 0x77, 0x66]
        );

        let rekor = kb.rekor.expect("rekor entry should be carried over");
        assert_eq!(rekor.log_index, 7777);
        assert_eq!(rekor.log_id, "01020304");
        assert_eq!(rekor.integrated_time, 1_700_000_000);
        assert_eq!(rekor.canonicalized_body, B64.encode(b"rekor-body-bytes"));
        assert_eq!(
            B64.decode(&rekor.set_b64).unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );

        let proof = rekor.inclusion_proof.expect("inclusion_proof carried over");
        assert_eq!(proof.tree_size, 12345);
        assert_eq!(proof.log_index, 7777);
        assert_eq!(proof.root_hash_hex, hex::encode([0x12u8; 32]));
        assert_eq!(proof.hashes_hex.len(), 2);
        assert_eq!(proof.hashes_hex[0], hex::encode([0x34u8; 32]));
    }

    #[test]
    fn rejects_protobuf_bundle_missing_signature() {
        let mut pb = sample_bundle();
        pb.message_signature = None;
        let bytes = pb.encode_to_vec();
        let err = KeylessBundle::from_protobuf_bundle(&bytes).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("missing message_signature"),
            "expected a missing-signature error, got: {msg}"
        );
    }

    #[test]
    fn accepts_single_certificate_field() {
        // Newer cosign bundles emit `certificate` (single cert) instead
        // of a `x509_certificate_chain`. Adapter must handle both.
        let mut pb = sample_bundle();
        let vm = pb.verification_material.as_mut().unwrap();
        vm.x509_certificate_chain = None;
        vm.certificate = Some(X509Certificate {
            raw_bytes: dummy_der(0xCC),
        });
        let bytes = pb.encode_to_vec();
        let kb = KeylessBundle::from_protobuf_bundle(&bytes).unwrap();
        assert!(kb.leaf_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(
            kb.chain_pem.is_none(),
            "single-cert form must not synthesise a chain"
        );
    }
}

// ---------------------------------------------------------------------------
// Rekor inclusion-proof verification (Phase 7.A).
// ---------------------------------------------------------------------------

/// Verify a Rekor Merkle inclusion proof per RFC 6962 §2.1.1.
///
/// Leaf hash is `SHA256(0x00 || canonicalized_body_bytes)` where the
/// body bytes are the base64-decoded value of [`RekorEntry::canonicalized_body`].
/// Internal-node hash is `SHA256(0x01 || left || right)`. The leaf index
/// the proof was issued for must equal `entry.log_index`, and the
/// proof's `tree_size` must be strictly greater than `log_index`.
fn verify_rekor_inclusion(
    entry: &RekorEntry,
    proof: &RekorInclusionProof,
) -> Result<(), TakoError> {
    if proof.log_index != entry.log_index {
        return Err(TakoError::Invalid(format!(
            "sigstore: rekor inclusion proof log_index ({}) does not match entry log_index ({})",
            proof.log_index, entry.log_index,
        )));
    }
    if proof.tree_size == 0 || entry.log_index >= proof.tree_size {
        return Err(TakoError::Invalid(format!(
            "sigstore: rekor inclusion proof: log_index {} out of range for tree_size {}",
            entry.log_index, proof.tree_size,
        )));
    }

    use sha2::{Digest, Sha256};

    let body_bytes = B64
        .decode(entry.canonicalized_body.trim())
        .map_err(|e| TakoError::Invalid(format!("sigstore: rekor body base64: {e}")))?;

    // Leaf hash: SHA256(0x00 || body)
    let mut h = Sha256::new();
    h.update([0x00u8]);
    h.update(&body_bytes);
    let mut r: [u8; 32] = h.finalize().into();

    // Decode each sibling hash from hex into [u8; 32].
    let path: Vec<[u8; 32]> = proof
        .hashes_hex
        .iter()
        .map(|s| {
            let raw = hex::decode(s.trim())
                .map_err(|e| TakoError::Invalid(format!("sigstore: rekor proof hex: {e}")))?;
            <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| {
                TakoError::Invalid(format!(
                    "sigstore: rekor proof hash wrong length (got {} bytes, want 32)",
                    raw.len()
                ))
            })
        })
        .collect::<Result<_, TakoError>>()?;

    let mut fnode = entry.log_index;
    let mut sn = proof.tree_size - 1;

    for p in &path {
        if sn == 0 {
            return Err(TakoError::Invalid(
                "sigstore: rekor inclusion proof too long".into(),
            ));
        }
        if fnode & 1 == 1 || fnode == sn {
            // r is right child; sibling is on the left.
            let mut h = Sha256::new();
            h.update([0x01u8]);
            h.update(p);
            h.update(r);
            r = h.finalize().into();
            // Skip the run of right-edge nodes that don't have a real
            // sibling (they propagate up unchanged).
            while fnode & 1 == 0 && fnode != 0 {
                fnode >>= 1;
                sn >>= 1;
            }
        } else {
            // r is left child; sibling is on the right.
            let mut h = Sha256::new();
            h.update([0x01u8]);
            h.update(r);
            h.update(p);
            r = h.finalize().into();
        }
        fnode >>= 1;
        sn >>= 1;
    }

    if sn != 0 {
        return Err(TakoError::Invalid(
            "sigstore: rekor inclusion proof too short".into(),
        ));
    }

    let expected_root = hex::decode(proof.root_hash_hex.trim())
        .map_err(|e| TakoError::Invalid(format!("sigstore: rekor root hex: {e}")))?;
    if expected_root.as_slice() != r {
        return Err(TakoError::Invalid(
            "sigstore: rekor inclusion proof: computed root does not match pinned root".into(),
        ));
    }
    Ok(())
}
