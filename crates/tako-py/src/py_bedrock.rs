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
    ///
    /// Phase 28.C — `url_prefetch` opts in to tako-side fetch of
    /// `ContentPart::ImageUrl` content (Bedrock's `ImageSource`
    /// has no URL variant; URL-source images require pre-fetch).
    /// Default is silent-drop. `url_prefetch_allow_http` allows
    /// `http://` URLs (off by default; HTTPS-only).
    /// `url_prefetch_timeout_secs` and `url_prefetch_max_bytes`
    /// override the 10s / 10 MiB defaults.
    #[new]
    #[pyo3(signature = (
        model,
        region=None,
        endpoint_url=None,
        profile_name=None,
        url_prefetch=false,
        url_prefetch_allow_http=false,
        url_prefetch_timeout_secs=None,
        url_prefetch_max_bytes=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        model: String,
        region: Option<String>,
        endpoint_url: Option<String>,
        profile_name: Option<String>,
        url_prefetch: bool,
        url_prefetch_allow_http: bool,
        url_prefetch_timeout_secs: Option<u64>,
        url_prefetch_max_bytes: Option<usize>,
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
        // Phase 28.C — URL pre-fetch knobs. Any of the four
        // url_prefetch_* flags enables pre-fetch.
        if url_prefetch {
            b = b.with_url_prefetch();
        }
        if url_prefetch_allow_http {
            b = b.with_url_prefetch_allow_http();
        }
        if let Some(secs) = url_prefetch_timeout_secs {
            b = b.with_url_prefetch_timeout(std::time::Duration::from_secs(secs));
        }
        if let Some(n) = url_prefetch_max_bytes {
            b = b.with_url_prefetch_max_bytes(n);
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
