//! `PyBedrock` — wraps `tako-providers-bedrock::BedrockProvider`.
//!
//! Bedrock's builder is async (loads the AWS credential chain), so the
//! constructor blocks the calling Python thread on the shared
//! pyo3-async-runtimes runtime. The GIL is released for the blocking
//! section.

use std::sync::Arc;

use pyo3::prelude::*;
use tako_providers_bedrock::BedrockProvider;

use crate::py_provider::{ProviderHandle, map_err};

#[pyclass(name = "Bedrock", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyBedrock {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyBedrock {
    /// Build a Bedrock provider.
    ///
    /// `region` defaults to whatever the AWS credential chain resolves
    /// (env, profile, IRSA). `endpoint_url` overrides the default
    /// Bedrock endpoint — useful for VPC-private endpoints or local
    /// mocks.
    #[new]
    #[pyo3(signature = (model, region=None, endpoint_url=None, profile_name=None))]
    fn new(
        py: Python<'_>,
        model: String,
        region: Option<String>,
        endpoint_url: Option<String>,
        profile_name: Option<String>,
    ) -> PyResult<Self> {
        let mut b = BedrockProvider::builder().model(model);
        if let Some(r) = region {
            b = b.region(r);
        }
        if let Some(u) = endpoint_url {
            b = b.endpoint_url(u);
        }
        if let Some(p) = profile_name {
            b = b.profile_name(p);
        }
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let provider = py.detach(|| rt.block_on(b.build())).map_err(map_err)?;
        Ok(Self {
            handle: ProviderHandle {
                inner: Arc::new(provider),
            },
        })
    }

    fn id(&self) -> &str {
        self.handle.inner.id()
    }
}
