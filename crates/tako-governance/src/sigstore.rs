//! Sigstore-based verification of MCP tool catalogues.
//!
//! For supply-chain integrity, an operator can pin the exact set of tools
//! an MCP server is permitted to expose by:
//!
//! 1. Authoring a JSON [`Catalogue`] of allowed [`ToolSchema`]s.
//! 2. Signing it with `cosign sign-blob --key cosign.key catalogue.json`,
//!    producing a base64-encoded signature.
//! 3. Distributing the catalogue + signature alongside the MCP server.
//! 4. Verifying both at startup with [`CatalogueVerifier`] before
//!    handing the schemas to `ToolRegistry::register_mcp`.
//!
//! Trust model for this Phase-4 landing: **keyed** — a pinned public
//! key (PEM, typically `cosign.pub`). Keyless verification (Fulcio cert
//! and Rekor offline bundle against the Sigstore public-good trust
//! root) is intentionally deferred; the same
//! [`CatalogueVerifier::verify`] return shape will lift onto a
//! bundle-based variant in a follow-up.
//!
//! Gated behind the `sigstore` Cargo feature so the heavy `sigstore`
//! crate (and its `aws-lc-rs` crypto backend) only land in the dep
//! tree when explicitly enabled.

use std::path::Path;

use ::sigstore::crypto::{CosignVerificationKey, Signature, SigningScheme};
use serde::{Deserialize, Serialize};
use tako_core::{TakoError, ToolSchema};

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
