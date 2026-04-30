//! Python entry points for the OpenAI-compatible HTTP server.
//!
//! The compat server boots in a background Tokio task; this module
//! exposes a `serve_openai_py(orch, host, port, tokens, auth, models)`
//! function that:
//!
//! 1. Extracts the orchestrator handle from a PyOrchestrator or
//!    PyConductor (any tako._native orchestrator).
//! 2. Builds an `AuthResolver` — by default a `StaticTokens` from the
//!    `tokens` dict, or one of the Phase 14.B real auth resolvers
//!    (`PyJwtAuth` / `PyOidcAuth` / `PyVaultAuth`) when `auth=...` is
//!    passed. Passing both `tokens` and `auth` is an error.
//! 3. Boots the server on the shared pyo3-async-runtimes Tokio runtime.
//! 4. Returns the bound URL string so the caller knows where it landed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_compat::{AuthResolver, ServeConfig, StaticTokens, serve_openai};
use tako_core::Principal;
use tako_orchestrator::Orchestrator;

use crate::py_provider::map_err;

/// Process-global handle to the running compat server. We support a
/// single live server per process; calling serve again after shutdown
/// is fine.
static SERVER: OnceLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();

fn server_slot() -> &'static Mutex<Option<tokio::task::JoinHandle<()>>> {
    SERVER.get_or_init(|| Mutex::new(None))
}

#[pyfunction]
#[pyo3(signature = (orch, host="127.0.0.1", port=8080, tokens=None, auth=None, models=None))]
pub fn serve_openai_py(
    py: Python<'_>,
    orch: Py<PyAny>,
    host: &str,
    port: u16,
    tokens: Option<HashMap<String, (String, String)>>,
    auth: Option<Py<PyAny>>,
    models: Option<Vec<String>>,
) -> PyResult<String> {
    let agent = extract_orchestrator(py, &orch)?;

    if tokens.is_some() && auth.is_some() {
        return Err(PyValueError::new_err(
            "pass either `tokens` (dev) or `auth` (real resolver), not both",
        ));
    }

    let auth_resolver: Arc<dyn AuthResolver> = if let Some(py_auth) = auth {
        extract_auth_resolver(py, &py_auth)?
    } else if let Some(map) = tokens {
        let mut t = StaticTokens::new();
        for (token, (tenant, user)) in map {
            t = t.with(token, Principal::new(tenant, user));
        }
        Arc::new(t)
    } else {
        // Default: a single dev token so smoke tests work without
        // credential setup. Production callers should always pass
        // their own map or an `auth=` resolver.
        Arc::new(StaticTokens::new().with("dev-token", Principal::new("anonymous", "anonymous")))
    };

    let config = ServeConfig {
        host: host.to_string(),
        port,
        models: models.unwrap_or_else(|| vec!["tako-default".into()]),
    };

    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let (addr, handle) = py
        .detach(|| rt.block_on(serve_openai(agent, auth_resolver, config)))
        .map_err(map_err)?;

    let mut slot = server_slot()
        .lock()
        .map_err(|e| PyValueError::new_err(format!("server slot poisoned: {e}")))?;
    if let Some(prev) = slot.take() {
        prev.abort();
    }
    *slot = Some(handle);

    Ok(format!("http://{addr}"))
}

#[pyfunction]
pub fn shutdown_compat_py() -> PyResult<()> {
    let mut slot = server_slot()
        .lock()
        .map_err(|e| PyValueError::new_err(format!("server slot poisoned: {e}")))?;
    if let Some(handle) = slot.take() {
        handle.abort();
    }
    Ok(())
}

fn extract_orchestrator(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn Orchestrator>> {
    let bound = obj.bind(py);
    if let Ok(o) = bound.cast::<crate::py_orchestrator::PyOrchestrator>() {
        return Ok(o.borrow().inner_arc());
    }
    if let Ok(o) = bound.cast::<crate::py_conductor::PyConductor>() {
        return Ok(o.borrow().inner_arc());
    }
    Err(PyValueError::new_err(
        "expected a tako._native.Orchestrator or Conductor",
    ))
}

/// Phase 14.B — downcast `auth` to one of the supported resolver
/// pyclasses. Each variant is gated on its `auth-*` cargo feature so
/// default wheels still build without the optional dep trees.
fn extract_auth_resolver(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn AuthResolver>> {
    let bound = obj.bind(py);
    #[cfg(feature = "auth-jwt")]
    if let Ok(a) = bound.cast::<PyJwtAuth>() {
        return Ok(a.borrow().inner.clone());
    }
    #[cfg(feature = "auth-oidc")]
    if let Ok(a) = bound.cast::<PyOidcAuth>() {
        return Ok(a.borrow().inner.clone());
    }
    #[cfg(feature = "auth-vault")]
    if let Ok(a) = bound.cast::<PyVaultAuth>() {
        return Ok(a.borrow().inner.clone());
    }
    let _ = bound;
    Err(PyValueError::new_err(
        "auth must be one of: tako._native.JwtAuth, OidcAuth, VaultAuth (build the wheel with the matching auth-* feature)",
    ))
}

// ---------------------------------------------------------------------------
// Phase 14.B — JwtAuth pyclass.
// ---------------------------------------------------------------------------

#[cfg(feature = "auth-jwt")]
#[pyclass(name = "JwtAuth", module = "tako._native")]
pub struct PyJwtAuth {
    inner: Arc<dyn AuthResolver>,
}

#[cfg(feature = "auth-jwt")]
impl std::fmt::Debug for PyJwtAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtAuth").finish_non_exhaustive()
    }
}

#[cfg(feature = "auth-jwt")]
fn finish_jwt(
    mut r: tako_compat::JwtAuthResolver,
    audience: Option<String>,
    issuer: Option<String>,
    tenant_claim: Option<String>,
    user_claim: Option<String>,
    roles_claim: Option<String>,
) -> PyJwtAuth {
    if let Some(a) = audience {
        r = r.with_audience(a);
    }
    if let Some(i) = issuer {
        r = r.with_issuer(i);
    }
    if let (Some(t), Some(u), Some(rc)) = (
        tenant_claim.as_ref(),
        user_claim.as_ref(),
        roles_claim.as_ref(),
    ) {
        r = r.with_claims(t, u, rc);
    }
    PyJwtAuth { inner: Arc::new(r) }
}

#[cfg(feature = "auth-jwt")]
#[pymethods]
impl PyJwtAuth {
    /// HS256 with a shared secret.
    #[staticmethod]
    #[pyo3(signature = (secret, *, audience=None, issuer=None, tenant_claim=None, user_claim=None, roles_claim=None))]
    fn hs256(
        secret: &[u8],
        audience: Option<String>,
        issuer: Option<String>,
        tenant_claim: Option<String>,
        user_claim: Option<String>,
        roles_claim: Option<String>,
    ) -> Self {
        finish_jwt(
            tako_compat::JwtAuthResolver::hs256(secret),
            audience,
            issuer,
            tenant_claim,
            user_claim,
            roles_claim,
        )
    }

    /// RS256 against an RSA public-key PEM.
    #[staticmethod]
    #[pyo3(signature = (pem, *, audience=None, issuer=None, tenant_claim=None, user_claim=None, roles_claim=None))]
    fn rs256_from_pem(
        pem: &[u8],
        audience: Option<String>,
        issuer: Option<String>,
        tenant_claim: Option<String>,
        user_claim: Option<String>,
        roles_claim: Option<String>,
    ) -> PyResult<Self> {
        let r = tako_compat::JwtAuthResolver::rs256_from_pem(pem).map_err(map_err)?;
        Ok(finish_jwt(
            r,
            audience,
            issuer,
            tenant_claim,
            user_claim,
            roles_claim,
        ))
    }

    /// ES256 against an EC public-key PEM.
    #[staticmethod]
    #[pyo3(signature = (pem, *, audience=None, issuer=None, tenant_claim=None, user_claim=None, roles_claim=None))]
    fn es256_from_pem(
        pem: &[u8],
        audience: Option<String>,
        issuer: Option<String>,
        tenant_claim: Option<String>,
        user_claim: Option<String>,
        roles_claim: Option<String>,
    ) -> PyResult<Self> {
        let r = tako_compat::JwtAuthResolver::es256_from_pem(pem).map_err(map_err)?;
        Ok(finish_jwt(
            r,
            audience,
            issuer,
            tenant_claim,
            user_claim,
            roles_claim,
        ))
    }
}

// ---------------------------------------------------------------------------
// Phase 14.B — OidcAuth pyclass.
// ---------------------------------------------------------------------------

#[cfg(feature = "auth-oidc")]
#[pyclass(name = "OidcAuth", module = "tako._native")]
pub struct PyOidcAuth {
    inner: Arc<dyn AuthResolver>,
}

#[cfg(feature = "auth-oidc")]
impl std::fmt::Debug for PyOidcAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuth").finish_non_exhaustive()
    }
}

#[cfg(feature = "auth-oidc")]
#[pymethods]
impl PyOidcAuth {
    /// Async constructor: discover an OIDC provider at `issuer` and
    /// require the supplied `audience` on every incoming token.
    #[staticmethod]
    fn discover<'py>(
        py: Python<'py>,
        issuer: String,
        audience: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let r = tako_compat::OidcAuthResolver::discover(&issuer, &audience)
                .await
                .map_err(map_err)?;
            Ok(PyOidcAuth { inner: Arc::new(r) })
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 14.B — VaultAuth pyclass.
// ---------------------------------------------------------------------------

#[cfg(feature = "auth-vault")]
#[pyclass(name = "VaultAuth", module = "tako._native")]
pub struct PyVaultAuth {
    inner: Arc<dyn AuthResolver>,
}

#[cfg(feature = "auth-vault")]
impl std::fmt::Debug for PyVaultAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultAuth").finish_non_exhaustive()
    }
}

#[cfg(feature = "auth-vault")]
#[pymethods]
impl PyVaultAuth {
    /// Sync constructor. `addr` looks like `http://127.0.0.1:8200`;
    /// `token` is the Vault token tako uses to authenticate to Vault
    /// itself (separate from user bearer tokens). Vault token rotation
    /// is out of scope — see the Rust-side rustdoc.
    #[new]
    fn new(addr: &str, token: &str) -> PyResult<Self> {
        let r = tako_compat::VaultAuthResolver::new(addr, token).map_err(map_err)?;
        Ok(Self { inner: Arc::new(r) })
    }
}
