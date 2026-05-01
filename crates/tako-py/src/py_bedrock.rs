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
    ///
    /// Phase 29.C — `url_prefetch_allow_private_ips` opts out of
    /// the default-on private-IP blocklist (loopback / RFC 1918
    /// / link-local / multicast / IPv6 unique-local + link-local
    /// + IPv4-mapped variants). Operators with deployment-level
    /// egress filtering can flip this on for internal artifact
    /// servers behind private addresses. Does NOT auto-enable
    /// `url_prefetch=True`.
    ///
    /// Phase 30.C — `url_prefetch_allow_hosts` is a per-host
    /// allowlist that bypasses the private-IP blocklist for
    /// specific hostnames only (scheme / timeout / size / MIME
    /// checks all still apply). Useful for permitting an internal
    /// artifact registry on a private RFC 1918 address while
    /// keeping the rest of the blocklist active. Pass `None` for
    /// no allowlist (default), or a list of hostnames to allow.
    #[new]
    #[pyo3(signature = (
        model,
        region=None,
        endpoint_url=None,
        profile_name=None,
        url_prefetch=false,
        url_prefetch_allow_http=false,
        url_prefetch_allow_private_ips=false,
        url_prefetch_allow_hosts=None,
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
        url_prefetch_allow_private_ips: bool,
        url_prefetch_allow_hosts: Option<Vec<String>>,
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
        // Phase 29.C — opt-out for private-IP blocklist.
        if url_prefetch_allow_private_ips {
            b = b.with_url_prefetch_allow_private_ips();
        }
        // Phase 30.C — per-host allowlist.
        if let Some(hosts) = url_prefetch_allow_hosts {
            for host in hosts {
                b = b.with_url_prefetch_allow_host(host);
            }
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
