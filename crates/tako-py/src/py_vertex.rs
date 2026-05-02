//! `PyVertex` — wraps `tako-providers-vertex::VertexProvider`.

use std::sync::Arc;
use std::time::Duration;

use pyo3::prelude::*;
use tako_providers_vertex::VertexProvider;

use crate::py_provider::{ProviderHandle, map_err};

#[pyclass(name = "Vertex", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyVertex {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyVertex {
    /// Build a Vertex AI (Gemini) provider.
    ///
    /// `access_token` is required; pass either a literal token or
    /// `"$ENV:VAR"` to read from the environment. The provider does not
    /// refresh tokens — wire your own credential source (gcloud, gcp_auth,
    /// the metadata server) and rebuild the provider before tokens expire.
    #[new]
    #[pyo3(signature = (project_id, model, access_token, location=None, endpoint_url=None, timeout_secs=None))]
    fn new(
        project_id: &str,
        model: &str,
        access_token: &str,
        location: Option<&str>,
        endpoint_url: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> PyResult<Self> {
        let mut b = VertexProvider::builder()
            .project_id(project_id)
            .model(model)
            .access_token(access_token);
        if let Some(l) = location {
            b = b.location(l);
        }
        if let Some(u) = endpoint_url {
            b = b.endpoint_url(u);
        }
        if let Some(t) = timeout_secs {
            b = b.timeout(Duration::from_secs(t));
        }
        let p = b.build().map_err(map_err)?;
        Ok(Self {
            handle: ProviderHandle { inner: Arc::new(p) },
        })
    }

    fn id(&self) -> &str {
        self.handle.inner.id()
    }
}
