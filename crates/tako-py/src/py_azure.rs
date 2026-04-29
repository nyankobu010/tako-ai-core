//! `PyAzureOpenAi` — wraps `tako-providers-azure-openai::AzureOpenAiProvider`.

use std::sync::Arc;
use std::time::Duration;

use pyo3::prelude::*;
use tako_providers_azure_openai::AzureOpenAiProvider;

use crate::py_provider::{ProviderHandle, map_err};

#[pyclass(name = "AzureOpenAi", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyAzureOpenAi {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyAzureOpenAi {
    /// Build an Azure OpenAI provider.
    ///
    /// `endpoint` is the Azure resource endpoint (e.g.
    /// `https://my-resource.openai.azure.com`). `deployment` is the Azure
    /// deployment name (a user-defined alias mapping to a model). `api_key`
    /// is required; pass either the literal key or `"$ENV:AZURE_OPENAI_API_KEY"`.
    #[new]
    #[pyo3(signature = (endpoint, deployment, api_key, api_version=None, timeout_secs=None))]
    fn new(
        endpoint: &str,
        deployment: &str,
        api_key: &str,
        api_version: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> PyResult<Self> {
        let mut b = AzureOpenAiProvider::builder()
            .endpoint(endpoint)
            .deployment(deployment)
            .api_key(api_key);
        if let Some(v) = api_version {
            b = b.api_version(v);
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
