//! Bindings for `tako_governance::CatalogueVerifier` (Phase 4.G) and
//! `tako_governance::KeylessVerifier` (Phase 5.A).
//!
//! Gated behind the `sigstore` Cargo feature; the underlying crate dep
//! arrives via `tako-governance/sigstore` only when this feature is
//! enabled.
#![cfg(feature = "sigstore")]

use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_governance::sigstore::{Catalogue, CatalogueVerifier, IdentityPolicy, KeylessVerifier};

use crate::py_provider::map_err;

/// Verifies cosign-signed MCP tool catalogue manifests against a
/// pinned public key (PEM, typically `cosign.pub`).
///
/// Construct with `CatalogueVerifier(pem_bytes)` or
/// `CatalogueVerifier.from_pem_path(path)`. Call `.verify(manifest,
/// signature)` to check the signature; on success returns a
/// `(server: Optional[str], tools_json: str)` tuple — the JSON of the
/// `tools` array round-tripped through the server's pinned schema.
#[pyclass(name = "CatalogueVerifier", module = "tako._native")]
pub struct PyCatalogueVerifier {
    inner: Arc<CatalogueVerifier>,
}

impl std::fmt::Debug for PyCatalogueVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyCatalogueVerifier")
            .finish_non_exhaustive()
    }
}

#[pymethods]
impl PyCatalogueVerifier {
    /// Build a verifier from PEM-encoded public-key bytes (pass
    /// `cosign.pub`'s contents).
    #[new]
    fn new(pem: &[u8]) -> PyResult<Self> {
        let v = CatalogueVerifier::from_pem(pem).map_err(map_err)?;
        Ok(Self { inner: Arc::new(v) })
    }

    /// Convenience: read the PEM from a filesystem path.
    #[staticmethod]
    fn from_pem_path(path: &str) -> PyResult<Self> {
        let v = CatalogueVerifier::from_pem_path(path).map_err(map_err)?;
        Ok(Self { inner: Arc::new(v) })
    }

    /// Verify `signature` over `manifest` and return
    /// `(server, tools_json)`. The Python facade re-parses
    /// `tools_json` into typed `tako.ToolSchema` objects.
    fn verify(&self, manifest: &[u8], signature: &[u8]) -> PyResult<(Option<String>, String)> {
        let cat: Catalogue = self.inner.verify(manifest, signature).map_err(map_err)?;
        let tools_json = serde_json::to_string(&cat.tools)
            .map_err(|e| PyValueError::new_err(format!("serialise tools: {e}")))?;
        Ok((cat.server, tools_json))
    }

    fn __repr__(&self) -> String {
        "CatalogueVerifier(...)".to_string()
    }
}

/// Verifies cosign keyless-style bundles against a pinned identity
/// policy (OIDC issuer + SAN match).
///
/// Construct with `KeylessVerifier(issuer, san, *, san_is_regex=False)`.
/// Call `.verify_bundle(manifest, bundle)` where `bundle` is a JSON
/// payload of shape `{"leaf_cert_pem": "...", "signature_b64": "..."}`.
/// Returns a `(server: Optional[str], tools_json: str)` tuple matching
/// `CatalogueVerifier.verify`.
#[pyclass(name = "KeylessVerifier", module = "tako._native")]
pub struct PyKeylessVerifier {
    inner: Arc<KeylessVerifier>,
}

impl std::fmt::Debug for PyKeylessVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyKeylessVerifier").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyKeylessVerifier {
    /// Build a verifier from an `IdentityPolicy`.
    ///
    /// `issuer` is the expected OIDC issuer URI from the leaf cert's
    /// Fulcio extension. `san` is matched against the cert's SAN — by
    /// default an exact-string match; set `san_is_regex=True` to treat
    /// `san` as an anchored regex pattern.
    #[new]
    #[pyo3(signature = (issuer, san, *, san_is_regex=false))]
    fn new(issuer: &str, san: &str, san_is_regex: bool) -> PyResult<Self> {
        let policy = if san_is_regex {
            IdentityPolicy::regex(issuer, san).map_err(map_err)?
        } else {
            IdentityPolicy::exact(issuer, san)
        };
        Ok(Self {
            inner: Arc::new(KeylessVerifier::new(policy)),
        })
    }

    /// Verify a tako keyless bundle and return `(server, tools_json)`.
    fn verify_bundle(&self, manifest: &[u8], bundle: &[u8]) -> PyResult<(Option<String>, String)> {
        let cat: Catalogue = self
            .inner
            .verify_bundle(manifest, bundle)
            .map_err(map_err)?;
        let tools_json = serde_json::to_string(&cat.tools)
            .map_err(|e| PyValueError::new_err(format!("serialise tools: {e}")))?;
        Ok((cat.server, tools_json))
    }

    fn __repr__(&self) -> String {
        "KeylessVerifier(...)".to_string()
    }
}
