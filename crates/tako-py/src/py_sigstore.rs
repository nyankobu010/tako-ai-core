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
use tako_governance::sigstore::{
    Catalogue, CatalogueVerifier, IdentityPolicy, KeylessVerifier, TrustRoot,
};

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
    /// Build a verifier from an `IdentityPolicy` plus optional
    /// chain-of-trust and Rekor-SET enforcement.
    ///
    /// `issuer` is the expected OIDC issuer URI from the leaf cert's
    /// Fulcio extension. `san` is matched against the cert's SAN — by
    /// default an exact-string match; set `san_is_regex=True` to treat
    /// `san` as an anchored regex pattern.
    ///
    /// `trust_root` is an optional `tako._native.TrustRoot` instance.
    /// When set, every cert in the bundle's `chain_pem` plus the leaf
    /// is signature-validated and must terminate at one of the trust
    /// root's roots. Without `trust_root`, chain validation is skipped
    /// (v0.6.0 behaviour).
    ///
    /// `rekor_public_key_pem` is an optional PEM (typically Rekor's
    /// public-good ECDSA-P256 key). When set and the bundle carries a
    /// `rekor` field, the SET is verified against the key.
    #[new]
    #[pyo3(signature = (issuer, san, *, san_is_regex=false, trust_root=None, rekor_public_key_pem=None))]
    fn new(
        py: Python<'_>,
        issuer: &str,
        san: &str,
        san_is_regex: bool,
        trust_root: Option<Py<PyAny>>,
        rekor_public_key_pem: Option<Vec<u8>>,
    ) -> PyResult<Self> {
        let policy = if san_is_regex {
            IdentityPolicy::regex(issuer, san).map_err(map_err)?
        } else {
            IdentityPolicy::exact(issuer, san)
        };
        let mut v = KeylessVerifier::new(policy);
        if let Some(tr) = trust_root {
            let tr_ref: PyRef<'_, PyTrustRoot> = tr.extract(py).map_err(|_| {
                PyValueError::new_err("trust_root must be a tako._native.TrustRoot")
            })?;
            v = v.with_trust_root((*tr_ref.inner).clone());
        }
        if let Some(pem) = rekor_public_key_pem {
            v = v.with_rekor_key(&pem).map_err(map_err)?;
        }
        Ok(Self { inner: Arc::new(v) })
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

/// Operator-pinned set of trust anchors (root + intermediate CAs).
/// Build with `TrustRoot(roots_pem, intermediates_pem=None)` or
/// `TrustRoot.from_paths(roots_path, intermediates_path=None)`.
#[pyclass(name = "TrustRoot", module = "tako._native")]
pub struct PyTrustRoot {
    inner: Arc<TrustRoot>,
}

impl std::fmt::Debug for PyTrustRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyTrustRoot").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyTrustRoot {
    #[new]
    #[pyo3(signature = (roots_pem, intermediates_pem=None))]
    fn new(roots_pem: &[u8], intermediates_pem: Option<&[u8]>) -> PyResult<Self> {
        let tr = TrustRoot::from_pem(roots_pem, intermediates_pem).map_err(map_err)?;
        Ok(Self {
            inner: Arc::new(tr),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (roots_path, intermediates_path=None))]
    fn from_paths(roots_path: &str, intermediates_path: Option<&str>) -> PyResult<Self> {
        let tr = TrustRoot::from_paths(roots_path, intermediates_path).map_err(map_err)?;
        Ok(Self {
            inner: Arc::new(tr),
        })
    }

    fn __repr__(&self) -> String {
        "TrustRoot(...)".to_string()
    }
}
