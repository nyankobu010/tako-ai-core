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

/// Phase 36 — parse the per-child short-circuit policy alias
/// passed from Python into the strongly-typed
/// [`tako_compat::ChildShortCircuitPolicy`]. Case-insensitive;
/// accepts both `snake_case` and `kebab-case` for symmetry with
/// the Phase 24 / 25 mTLS auth-method aliases.
fn parse_child_short_circuit_policy(s: &str) -> PyResult<tako_compat::ChildShortCircuitPolicy> {
    use tako_compat::ChildShortCircuitPolicy as P;
    let normalised = s.trim().to_ascii_lowercase().replace('-', "_");
    match normalised.as_str() {
        "inherit" => Ok(P::Inherit),
        "always_fall_through" => Ok(P::AlwaysFallThrough),
        "transport_only" => Ok(P::TransportOnly),
        "all_infrastructure" => Ok(P::AllInfrastructure),
        other => Err(PyValueError::new_err(format!(
            "unknown ChainedAuth child policy {other:?}; expected one of \
             \"inherit\" / \"always_fall_through\" / \"transport_only\" / \"all_infrastructure\" \
             (kebab-case variants accepted)"
        ))),
    }
}

/// Phase 14.B / 21.B — downcast `auth` to one of the supported
/// resolver pyclasses. JWT / OIDC / Vault are gated on their
/// `auth-*` cargo features so default wheels still build without
/// the optional dep trees; `ChainedAuth` (Phase 21.B) is always-on.
fn extract_auth_resolver(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<Arc<dyn AuthResolver>> {
    let bound = obj.bind(py);
    #[cfg(feature = "auth-jwt")]
    if let Ok(a) = bound.cast::<PyJwtAuth>() {
        return Ok(a.borrow().inner.clone());
    }
    #[cfg(feature = "auth-oidc")]
    if let Ok(a) = bound.cast::<PyOidcAuth>() {
        let oidc = a.borrow();
        return Ok(Arc::clone(&oidc.inner) as Arc<dyn AuthResolver>);
    }
    #[cfg(feature = "auth-vault")]
    if let Ok(a) = bound.cast::<PyVaultAuth>() {
        let vault = a.borrow();
        return Ok(Arc::clone(&vault.inner) as Arc<dyn AuthResolver>);
    }
    // Phase 21.B — composite resolver. Recursive: a `ChainedAuth`
    // can contain another `ChainedAuth` (the chained.rs
    // `chained_can_nest` test pins this on the Rust side).
    if let Ok(a) = bound.cast::<PyChainedAuth>() {
        let chained = a.borrow();
        return Ok(Arc::clone(&chained.inner) as Arc<dyn AuthResolver>);
    }
    let _ = bound;
    Err(PyValueError::new_err(
        "auth must be one of: tako._native.JwtAuth, OidcAuth, VaultAuth, ChainedAuth (build the wheel with the matching auth-* feature for JWT / OIDC / Vault)",
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
// Phase 15.B.2 — `with_introspection` / `with_introspection_uri`.
// ---------------------------------------------------------------------------

#[cfg(feature = "auth-oidc")]
#[pyclass(name = "OidcAuth", module = "tako._native")]
pub struct PyOidcAuth {
    inner: Arc<tako_compat::OidcAuthResolver>,
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

    /// Phase 15.B.2 — enable RFC 7662 token introspection using the
    /// `introspection_endpoint` advertised by the issuer's discovery
    /// doc. Returns a NEW `OidcAuth` instance (immutable builder);
    /// raises `ValueError` if the issuer didn't advertise an endpoint.
    #[pyo3(signature = (client_id, client_secret=None))]
    fn with_introspection(
        &self,
        client_id: String,
        client_secret: Option<String>,
    ) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection(client_id, client_secret)
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 15.B.2 — enable RFC 7662 token introspection with an
    /// explicit endpoint URL (bypasses discovery). Infallible.
    #[pyo3(signature = (uri, client_id, client_secret=None))]
    fn with_introspection_uri(
        &self,
        uri: String,
        client_id: String,
        client_secret: Option<String>,
    ) -> Self {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned.with_introspection_uri(uri, client_id, client_secret);
        PyOidcAuth { inner: Arc::new(r) }
    }

    /// Phase 16.B.2 / 17.B / 18.A — set the RFC 7662 §2.1
    /// introspection-endpoint auth method. Accepts case-insensitive
    /// aliases: `"basic"` / `"client_secret_basic"` (default; HTTP
    /// Basic header), `"post"` / `"client_secret_post"` (credentials
    /// in form body), `"jwt"` / `"client_secret_jwt"` (Phase 17.B;
    /// HS256-signed JWT client assertion per RFC 7521 / 7523), or
    /// `"private_key_jwt"` / `"private-key-jwt"` (Phase 18.A;
    /// asymmetric RS256 / ES256 / EdDSA JWT — requires a key
    /// loaded via one of the
    /// `with_introspection_jwt_*_pem` builders below).
    /// Any other value raises `ValueError`. Silent no-op when no
    /// introspection config has been attached yet.
    fn with_introspection_auth_method(&self, auth_method: &str) -> PyResult<Self> {
        let am = match auth_method.to_ascii_lowercase().as_str() {
            "basic" | "client_secret_basic" => {
                tako_compat::IntrospectionAuthMethod::ClientSecretBasic
            }
            "post" | "client_secret_post" => tako_compat::IntrospectionAuthMethod::ClientSecretPost,
            "jwt" | "client_secret_jwt" => tako_compat::IntrospectionAuthMethod::ClientSecretJwt,
            "private_key_jwt" | "private-key-jwt" => {
                tako_compat::IntrospectionAuthMethod::PrivateKeyJwt
            }
            // Phase 24.B — RFC 8705 §2.1 CA-backed mTLS. Three
            // case-insensitive aliases: spec name, kebab variant,
            // operator-friendly shorthand.
            "tls_client_auth" | "tls-client-auth" | "mtls" => {
                tako_compat::IntrospectionAuthMethod::TlsClientAuth
            }
            // Phase 25.B — RFC 8705 §2.2 self-signed mTLS.
            // Issuer matches the cert against a pre-registered
            // thumbprint or fingerprint instead of a CA chain.
            // Four case-insensitive aliases: spec name, kebab
            // variant, two operator-friendly shorthands.
            "self_signed_tls_client_auth"
            | "self-signed-tls-client-auth"
            | "self_signed_mtls"
            | "self-signed-mtls" => tako_compat::IntrospectionAuthMethod::SelfSignedTlsClientAuth,
            other => {
                return Err(PyValueError::new_err(format!(
                    "auth_method must be one of: 'basic' / 'client_secret_basic' / \
                     'post' / 'client_secret_post' / 'jwt' / 'client_secret_jwt' / \
                     'private_key_jwt' / 'tls_client_auth' / 'mtls' / \
                     'self_signed_tls_client_auth' / 'self_signed_mtls' \
                     (got {other:?})",
                )));
            }
        };
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned.with_introspection_auth_method(am);
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 17.A / 18.A — auto-select the introspection-endpoint
    /// auth method against the issuer's RFC 8414
    /// `introspection_endpoint_auth_methods_supported` list captured
    /// during discovery. Returns a NEW `OidcAuth`. Silent no-op
    /// (returns a clone) when no introspection config has been
    /// attached yet. Raises `ValueError` when discovery advertised
    /// a list with no supported variant (so the operator notices at
    /// builder time rather than at HTTP-401 from the introspection
    /// endpoint).
    ///
    /// Preference order (Phase 18.A):
    /// `private_key_jwt` (only when an asymmetric key is loaded via
    /// `with_introspection_jwt_*_pem`) →
    /// `client_secret_jwt` (only when a `client_secret` is
    /// configured — HS256 needs the symmetric key) →
    /// `client_secret_basic` → `client_secret_post`.
    fn with_introspection_auth_method_from_discovery(&self) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection_auth_method_from_discovery()
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 18.A — load an RSA private-key PEM (PKCS#8 or
    /// SEC1-style) and switch the introspection auth method to
    /// `private_key_jwt` (RFC 7521 / 7523, RS256). Returns a NEW
    /// `OidcAuth`. Silent no-op when no introspection config has
    /// been attached yet. Raises `ValueError` on PEM parse failure.
    fn with_introspection_jwt_rs256_pem(&self, pem: &[u8]) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection_jwt_rs256_pem(pem)
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 18.A — ES256 sibling of
    /// [`Self::with_introspection_jwt_rs256_pem`].
    fn with_introspection_jwt_es256_pem(&self, pem: &[u8]) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection_jwt_es256_pem(pem)
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 18.A — EdDSA sibling of
    /// [`Self::with_introspection_jwt_rs256_pem`].
    fn with_introspection_jwt_ed25519_pem(&self, pem: &[u8]) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection_jwt_ed25519_pem(pem)
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 24.B — load a client cert + private key from separate
    /// PEM blobs, build an mTLS-enabled HTTP client, and switch the
    /// introspection auth method to `tls_client_auth` (RFC 8705).
    /// Returns a NEW `OidcAuth`. Raises `ValueError` on PEM parse
    /// failure or `reqwest::Client` build failure.
    ///
    /// The `cert_pem` should be a PEM-encoded X.509 certificate
    /// (or chain); `key_pem` should be a PKCS#8 or SEC1-encoded
    /// private key matching the cert.
    fn with_introspection_mtls(&self, cert_pem: &[u8], key_pem: &[u8]) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection_mtls(cert_pem, key_pem)
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 24.B — convenience for combined PEM blobs (the common
    /// `cat cert.pem key.pem` form). Same behaviour as
    /// [`Self::with_introspection_mtls`] with the same blob passed
    /// twice.
    fn with_introspection_mtls_combined(&self, combined_pem: &[u8]) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection_mtls_combined(combined_pem)
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 25.B — load a client cert + private key, build an
    /// mTLS-enabled HTTP client, and switch the introspection
    /// auth method to RFC 8705 §2.2 `self_signed_tls_client_auth`.
    /// Identical wire shape to
    /// [`Self::with_introspection_mtls`]; the only difference is
    /// the auth-method enum variant. Returns a NEW `OidcAuth`.
    /// Raises `ValueError` on PEM parse / `Client` build failure.
    fn with_introspection_self_signed_mtls(
        &self,
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection_self_signed_mtls(cert_pem, key_pem)
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 25.B — convenience for combined PEM blobs (the
    /// common `cat cert.pem key.pem` form). Self-signed sibling
    /// of [`Self::with_introspection_mtls_combined`].
    fn with_introspection_self_signed_mtls_combined(&self, combined_pem: &[u8]) -> PyResult<Self> {
        let cloned: tako_compat::OidcAuthResolver = (*self.inner).clone();
        let r = cloned
            .with_introspection_self_signed_mtls_combined(combined_pem)
            .map_err(map_err)?;
        Ok(PyOidcAuth { inner: Arc::new(r) })
    }

    /// Phase 33 — atomically replace the mTLS identity used for
    /// OIDC introspection POSTs without rebuilding the resolver.
    /// Useful for cert rotation in long-running deployments
    /// (cert-manager webhook, Vault PKI rotation, filesystem
    /// watcher, periodic poll).
    ///
    /// Mutates state in place via internal mutability (does NOT
    /// return a new `OidcAuth`). The swap is atomic from the
    /// request handler's perspective: concurrent introspection
    /// POSTs either see the old Client or the new one — never a
    /// torn state.
    ///
    /// Raises `ValueError` when no prior `with_introspection_mtls`
    /// or `with_introspection_self_signed_mtls` call has been
    /// made (operator error rather than silent no-op). Raises
    /// `ValueError` on PEM parse / `Client` build failure AND
    /// leaves the previously installed Client unchanged.
    fn reload_mtls_identity(&self, cert_pem: &[u8], key_pem: &[u8]) -> PyResult<()> {
        self.inner
            .reload_mtls_identity(cert_pem, key_pem)
            .map_err(map_err)
    }

    /// Phase 33 — convenience for combined PEM blobs (the common
    /// `cat cert.pem key.pem` form). Mirrors
    /// [`Self::with_introspection_mtls_combined`]'s cadence.
    fn reload_mtls_identity_combined(&self, combined_pem: &[u8]) -> PyResult<()> {
        self.inner
            .reload_mtls_identity_combined(combined_pem)
            .map_err(map_err)
    }

    /// Phase 35 — spawn a background filesystem-watcher task that
    /// auto-calls `reload_mtls_identity` whenever the cert or key
    /// file changes on disk.
    ///
    /// Hold the returned `MtlsFsWatcher` for the lifetime of the
    /// resolver — typically a module-scope variable in
    /// production. Dropping the handle (or calling its
    /// `shutdown()` / using it as a context manager) stops the
    /// watcher cleanly.
    ///
    /// Raises `ValueError` when no prior `with_introspection_mtls`
    /// or `with_introspection_self_signed_mtls` call has been
    /// made, when either parent directory is missing, or when
    /// the platform's filesystem watcher cannot be installed
    /// (kernel limit, permission, unsupported filesystem).
    #[cfg(feature = "auth-mtls-fs-watch")]
    fn watch_mtls_files(
        &self,
        cert_path: std::path::PathBuf,
        key_path: std::path::PathBuf,
    ) -> PyResult<PyMtlsFsWatcher> {
        let watcher = self
            .inner
            .clone()
            .watch_mtls_files(cert_path, key_path)
            .map_err(map_err)?;
        Ok(PyMtlsFsWatcher {
            inner: Some(watcher),
        })
    }

    /// Phase 18.B — return the OIDC Session Management 1.0
    /// `end_session_endpoint` URL the issuer advertised at discovery
    /// time. `None` when the issuer doesn't implement OIDC Session
    /// Management.
    fn end_session_endpoint(&self) -> Option<String> {
        self.inner.end_session_endpoint().map(str::to_string)
    }

    /// Phase 18.B — build a logout URL per OIDC Session Management
    /// 1.0 §5. Returns `None` when the issuer didn't advertise
    /// `end_session_endpoint`. All params are optional; passing
    /// `None` for everything yields the bare endpoint URL.
    #[pyo3(signature = (id_token_hint=None, post_logout_redirect_uri=None, state=None))]
    fn build_logout_uri(
        &self,
        id_token_hint: Option<&str>,
        post_logout_redirect_uri: Option<&str>,
        state: Option<&str>,
    ) -> Option<String> {
        self.inner
            .build_logout_uri(id_token_hint, post_logout_redirect_uri, state)
    }
}

// ---------------------------------------------------------------------------
// Phase 14.B — VaultAuth pyclass.
// Phase 15.B.1 — `with_approle` / `with_kubernetes` / `with_kubernetes_in_pod`.
// ---------------------------------------------------------------------------

#[cfg(feature = "auth-vault")]
#[pyclass(name = "VaultAuth", module = "tako._native")]
pub struct PyVaultAuth {
    inner: Arc<tako_compat::VaultAuthResolver>,
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
    /// Static-token constructor. `addr` looks like
    /// `http://127.0.0.1:8200`; `token` is a fixed Vault token tako
    /// uses to authenticate to Vault itself. For dynamic rotation,
    /// use `with_approle` / `with_kubernetes` / `with_kubernetes_in_pod`.
    #[new]
    fn new(addr: &str, token: &str) -> PyResult<Self> {
        let r = tako_compat::VaultAuthResolver::new(addr, token).map_err(map_err)?;
        Ok(Self { inner: Arc::new(r) })
    }

    /// Phase 15.B.1 — AppRole-rotating Vault token. POSTs
    /// `{role_id, secret_id}` to `<addr>/v1/auth/approle/login`
    /// lazily on each request whose cached lease has expired.
    #[staticmethod]
    fn with_approle(addr: &str, role_id: &str, secret_id: &str) -> PyResult<Self> {
        let r = tako_compat::VaultAuthResolver::with_approle(addr, role_id, secret_id)
            .map_err(map_err)?;
        Ok(Self { inner: Arc::new(r) })
    }

    /// Phase 15.B.1 — Kubernetes-auth rotating Vault token. Reads the
    /// SA JWT from `jwt_path` on each (re-)auth so SA-token rotation
    /// is picked up.
    #[staticmethod]
    fn with_kubernetes(addr: &str, role: &str, jwt_path: &str) -> PyResult<Self> {
        let r = tako_compat::VaultAuthResolver::with_kubernetes(
            addr,
            role,
            std::path::PathBuf::from(jwt_path),
        )
        .map_err(map_err)?;
        Ok(Self { inner: Arc::new(r) })
    }

    /// Phase 15.B.1 — convenience constructor for in-pod Kubernetes
    /// auth: `jwt_path` defaults to
    /// `/var/run/secrets/kubernetes.io/serviceaccount/token`.
    #[staticmethod]
    fn with_kubernetes_in_pod(addr: &str, role: &str) -> PyResult<Self> {
        let r =
            tako_compat::VaultAuthResolver::with_kubernetes_in_pod(addr, role).map_err(map_err)?;
        Ok(Self { inner: Arc::new(r) })
    }

    /// Phase 16.B.3 — set the Vault Enterprise namespace for every
    /// outgoing Vault request. The value is sent as the
    /// `X-Vault-Namespace` header. Returns a NEW `VaultAuth`
    /// instance (immutable builder); chainable on top of any
    /// constructor (`new` / `with_approle` / `with_kubernetes` /
    /// `with_kubernetes_in_pod`) since namespace is orthogonal to
    /// auth method.
    fn with_namespace(&self, namespace: &str) -> Self {
        let cloned: tako_compat::VaultAuthResolver = (*self.inner).clone();
        let r = cloned.with_namespace(namespace);
        Self { inner: Arc::new(r) }
    }
}

// ---------------------------------------------------------------------------
// Phase 21.B — ChainedAuth pyclass.
// Always-on (no feature gate) — `ChainedAuthResolver` is itself
// always-on; only its children carry feature gates.
// ---------------------------------------------------------------------------

#[pyclass(name = "ChainedAuth", module = "tako._native")]
pub struct PyChainedAuth {
    inner: Arc<tako_compat::ChainedAuthResolver>,
}

impl std::fmt::Debug for PyChainedAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainedAuth")
            .field("len", &self.inner.len())
            .finish()
    }
}

#[pymethods]
impl PyChainedAuth {
    /// Phase 21.B — empty composite chain. `serve_openai(auth=...)`
    /// rejects an empty chain at request time
    /// (`TakoError::Invalid("chained auth: no resolvers
    /// configured")`); add at least one child via [`Self::with`]
    /// before passing to `serve_openai`.
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(tako_compat::ChainedAuthResolver::new()),
        }
    }

    /// Phase 21.B — append a child resolver. Returns a NEW
    /// `ChainedAuth` (immutable builder; matches the `OidcAuth` /
    /// `VaultAuth` cadence). Accepts any `JwtAuth`, `OidcAuth`,
    /// `VaultAuth`, or `ChainedAuth` (recursive composition); the
    /// underlying [`extract_auth_resolver`] helper does the
    /// downcast.
    ///
    /// Children are tried in append order at request time; the
    /// first to return a Principal short-circuits. Any error from
    /// a child falls through to the next.
    ///
    /// Named `then(child)` not `with(child)` because `with` is a
    /// Python keyword — `chain.with(...)` would be a SyntaxError.
    /// `then` reads naturally ("try `self`, then `child` if that
    /// fails") and matches the JS `Promise.then` / Rust `Future`
    /// `.then(...)` idiom.
    fn then(&self, py: Python<'_>, child: Py<PyAny>) -> PyResult<Self> {
        let child = extract_auth_resolver(py, &child)?;
        let cloned: tako_compat::ChainedAuthResolver = (*self.inner).clone();
        let next = cloned.then(child);
        Ok(Self {
            inner: Arc::new(next),
        })
    }

    /// Phase 36 — append a child WITH a per-child
    /// short-circuit-policy override. The chain-wide policy
    /// (set by `with_short_circuit_on_transport_error` /
    /// `with_short_circuit_on_infrastructure_errors`) still
    /// applies to every child whose own override is `"inherit"`
    /// (the [`Self::then`] default).
    ///
    /// `policy` accepts the case-insensitive aliases:
    ///
    /// - `"inherit"` (same as `then(child)`)
    /// - `"always_fall_through"` / `"always-fall-through"` —
    ///   override: every error from this child falls through.
    ///   Useful for graceful-tail fallbacks (in-process static
    ///   tokens that must keep serving).
    /// - `"transport_only"` / `"transport-only"` — override:
    ///   short-circuit only on `Transport` from this child.
    ///   Narrower than chain-wide infra.
    /// - `"all_infrastructure"` / `"all-infrastructure"` —
    ///   override: short-circuit on the four infrastructure
    ///   variants. Broader than chain-wide transport-only.
    ///
    /// Raises `ValueError` on an unrecognised policy string.
    fn then_with_short_circuit(
        &self,
        py: Python<'_>,
        child: Py<PyAny>,
        policy: &str,
    ) -> PyResult<Self> {
        let child = extract_auth_resolver(py, &child)?;
        let parsed = parse_child_short_circuit_policy(policy)?;
        let cloned: tako_compat::ChainedAuthResolver = (*self.inner).clone();
        let next = cloned.then_with_short_circuit(child, parsed);
        Ok(Self {
            inner: Arc::new(next),
        })
    }

    /// Phase 21.B — number of children appended via
    /// [`Self::with`]. `len(chain)` from Python.
    fn __len__(&self) -> usize {
        self.inner.len()
    }

    /// Phase 26.B — opt in to fail-fast on transport errors.
    /// Returns a NEW `ChainedAuth` (immutable builder; matches
    /// the `then(...)` cadence). Idempotent.
    ///
    /// When enabled, a [`tako_compat::TakoError::Transport`] from
    /// any child halts the chain immediately instead of falling
    /// through to the next child. Useful for the common
    /// "OIDC bearer OR static API key" pattern: when the OIDC
    /// issuer is unreachable, surface the actionable
    /// `"transport error: ..."` instead of a misleading
    /// `"unknown bearer token"` from a fallback resolver.
    fn with_short_circuit_on_transport_error(&self) -> Self {
        let cloned: tako_compat::ChainedAuthResolver = (*self.inner).clone();
        let next = cloned.with_short_circuit_on_transport_error();
        Self {
            inner: Arc::new(next),
        }
    }

    /// Phase 26.B — accessor: returns `True` for both the Phase
    /// 26 narrower policy
    /// (`with_short_circuit_on_transport_error`) and the Phase 27
    /// broader policy
    /// (`with_short_circuit_on_infrastructure_errors`) — both
    /// short-circuit on `TakoError::Transport`.
    fn short_circuits_on_transport_error(&self) -> bool {
        self.inner.short_circuits_on_transport_error()
    }

    /// Phase 27.B — opt in to broader fail-fast: short-circuit
    /// on infrastructure / operator-set-guard errors that
    /// fall-through would mask:
    ///
    /// - `TakoError::Transport` (network failure)
    /// - `TakoError::RateLimited` (operator-side limit)
    /// - `TakoError::CircuitOpen` (failsafe circuit)
    /// - `TakoError::BudgetExhausted` (operator-set spend cap)
    ///
    /// Auth-decision errors (`TakoError::Invalid`,
    /// `PolicyDenied`) and vendor errors (`Provider`) continue to
    /// fall through. Returns a NEW `ChainedAuth` (immutable
    /// builder; idempotent). Last-write-wins between this and
    /// `with_short_circuit_on_transport_error` — the policy is
    /// overwritten, not merged.
    fn with_short_circuit_on_infrastructure_errors(&self) -> Self {
        let cloned: tako_compat::ChainedAuthResolver = (*self.inner).clone();
        let next = cloned.with_short_circuit_on_infrastructure_errors();
        Self {
            inner: Arc::new(next),
        }
    }

    /// Phase 27.B — accessor for the broader policy. Returns
    /// `True` only when
    /// `with_short_circuit_on_infrastructure_errors` was the most
    /// recent policy setter.
    fn short_circuits_on_infrastructure_errors(&self) -> bool {
        self.inner.short_circuits_on_infrastructure_errors()
    }
}

// ---------------------------------------------------------------------------
// Phase 35 — MtlsFsWatcher pyclass.
//
// Returned by `OidcAuth.watch_mtls_files(cert_path, key_path)`.
// Holds the background tokio task that re-reads + reloads the
// mTLS identity on filesystem changes. Drop / `shutdown` /
// `__exit__` stop the task cleanly.
// ---------------------------------------------------------------------------

#[cfg(feature = "auth-mtls-fs-watch")]
#[pyclass(name = "MtlsFsWatcher", module = "tako._native")]
pub struct PyMtlsFsWatcher {
    inner: Option<tako_compat::MtlsFsWatcher>,
}

#[cfg(feature = "auth-mtls-fs-watch")]
impl std::fmt::Debug for PyMtlsFsWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsFsWatcher")
            .field("running", &self.inner.is_some())
            .finish()
    }
}

#[cfg(feature = "auth-mtls-fs-watch")]
#[pymethods]
impl PyMtlsFsWatcher {
    /// Stop the watcher and await teardown of the background
    /// task. Idempotent; calling twice is a no-op the second
    /// time.
    fn shutdown(&mut self, py: Python<'_>) -> PyResult<()> {
        if let Some(w) = self.inner.take() {
            py.detach(|| {
                pyo3_async_runtimes::tokio::get_runtime().block_on(async {
                    w.shutdown().await;
                });
            });
        }
        Ok(())
    }

    /// Context-manager `__enter__`: returns `self`. Use as
    /// `with oidc.watch_mtls_files(cert, key) as w:` to scope
    /// the watcher to a block.
    fn __enter__(slf: PyRefMut<'_, Self>) -> PyRefMut<'_, Self> {
        slf
    }

    /// Context-manager `__exit__`: shuts the watcher down.
    /// Returns `False` so any in-flight exception still
    /// propagates.
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exc_type: Py<PyAny>,
        _exc: Py<PyAny>,
        _tb: Py<PyAny>,
    ) -> PyResult<bool> {
        self.shutdown(py)?;
        Ok(false)
    }

    fn __repr__(&self) -> String {
        format!(
            "MtlsFsWatcher(running={})",
            if self.inner.is_some() {
                "True"
            } else {
                "False"
            }
        )
    }
}
