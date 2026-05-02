//! Python bindings for the four cloud `SecretResolver` impls.
//!
//! Each class exposes an async `resolve(key)` that returns the resolved
//! secret as a Python `str`. Python callers should treat the returned
//! value as sensitive; we don't surface a `SecretString` Python type
//! today (the redaction guarantees only hold inside the Rust stack).

use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_core::TakoError;
use tako_governance::{
    AwsSecretsManagerResolver, AzureKeyVaultResolver, GcpSecretManagerResolver, SecretResolver,
    VaultResolver,
};

use crate::py_provider::map_err;

fn run_resolve<'py, R>(
    py: Python<'py>,
    resolver: Arc<R>,
    key: String,
) -> PyResult<Bound<'py, PyAny>>
where
    R: SecretResolver + ?Sized + 'static,
{
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let secret = resolver
            .resolve(&key)
            .await
            .map_err(|e: TakoError| PyValueError::new_err(e.to_string()))?;
        Ok(secret.into_inner())
    })
}

#[pyclass(name = "VaultResolver", module = "tako._native", from_py_object)]
#[derive(Clone)]
pub struct PyVaultResolver {
    inner: Arc<VaultResolver>,
}

impl std::fmt::Debug for PyVaultResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyVaultResolver").finish()
    }
}

#[pymethods]
impl PyVaultResolver {
    #[new]
    fn new(addr: &str, token: &str) -> PyResult<Self> {
        let r = VaultResolver::new(addr, token).map_err(map_err)?;
        Ok(Self { inner: Arc::new(r) })
    }

    fn resolve<'py>(&self, py: Python<'py>, key: String) -> PyResult<Bound<'py, PyAny>> {
        run_resolve(py, self.inner.clone(), key)
    }
}

#[pyclass(
    name = "AzureKeyVaultResolver",
    module = "tako._native",
    from_py_object
)]
#[derive(Clone)]
pub struct PyAzureKeyVaultResolver {
    inner: Arc<AzureKeyVaultResolver>,
}

impl std::fmt::Debug for PyAzureKeyVaultResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyAzureKeyVaultResolver").finish()
    }
}

#[pymethods]
impl PyAzureKeyVaultResolver {
    #[new]
    #[pyo3(signature = (vault_url, access_token, api_version=None))]
    fn new(vault_url: &str, access_token: &str, api_version: Option<&str>) -> PyResult<Self> {
        let r = match api_version {
            Some(v) => AzureKeyVaultResolver::with_api_version(vault_url, access_token, v),
            None => AzureKeyVaultResolver::new(vault_url, access_token),
        }
        .map_err(map_err)?;
        Ok(Self { inner: Arc::new(r) })
    }

    fn resolve<'py>(&self, py: Python<'py>, key: String) -> PyResult<Bound<'py, PyAny>> {
        run_resolve(py, self.inner.clone(), key)
    }
}

#[pyclass(
    name = "GcpSecretManagerResolver",
    module = "tako._native",
    from_py_object
)]
#[derive(Clone)]
pub struct PyGcpSecretManagerResolver {
    inner: Arc<GcpSecretManagerResolver>,
}

impl std::fmt::Debug for PyGcpSecretManagerResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyGcpSecretManagerResolver").finish()
    }
}

#[pymethods]
impl PyGcpSecretManagerResolver {
    #[new]
    #[pyo3(signature = (project_id, access_token, endpoint_url=None))]
    fn new(project_id: &str, access_token: &str, endpoint_url: Option<&str>) -> PyResult<Self> {
        let r = match endpoint_url {
            Some(u) => GcpSecretManagerResolver::with_endpoint(project_id, access_token, u),
            None => GcpSecretManagerResolver::new(project_id, access_token),
        }
        .map_err(map_err)?;
        Ok(Self { inner: Arc::new(r) })
    }

    fn resolve<'py>(&self, py: Python<'py>, key: String) -> PyResult<Bound<'py, PyAny>> {
        run_resolve(py, self.inner.clone(), key)
    }
}

#[pyclass(
    name = "AwsSecretsManagerResolver",
    module = "tako._native",
    from_py_object
)]
#[derive(Clone)]
pub struct PyAwsSecretsManagerResolver {
    inner: Arc<AwsSecretsManagerResolver>,
}

impl std::fmt::Debug for PyAwsSecretsManagerResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyAwsSecretsManagerResolver").finish()
    }
}

#[pymethods]
impl PyAwsSecretsManagerResolver {
    #[new]
    #[pyo3(signature = (region=None, profile_name=None, endpoint_url=None))]
    fn new(
        region: Option<&str>,
        profile_name: Option<&str>,
        endpoint_url: Option<&str>,
    ) -> PyResult<Self> {
        let mut r = AwsSecretsManagerResolver::new();
        if let Some(reg) = region {
            r = r.with_region(reg);
        }
        if let Some(p) = profile_name {
            r = r.with_profile(p);
        }
        if let Some(u) = endpoint_url {
            r = r.with_endpoint_url(u);
        }
        Ok(Self { inner: Arc::new(r) })
    }

    fn resolve<'py>(&self, py: Python<'py>, key: String) -> PyResult<Bound<'py, PyAny>> {
        run_resolve(py, self.inner.clone(), key)
    }
}
