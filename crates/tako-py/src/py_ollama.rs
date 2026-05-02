//! `PyOllama` — wraps `tako-providers-ollama::OllamaProvider`.
//!
//! Ollama's builder is sync (no async credential chain), so the
//! constructor calls `OllamaBuilder::build()` directly without
//! `py.detach + rt.block_on`. Mirror of [`crate::py_bedrock`]
//! cadence otherwise.

use std::sync::Arc;

use pyo3::prelude::*;
use tako_providers_ollama::OllamaProvider;

use crate::py_provider::{ProviderHandle, map_err};

#[pyclass(name = "Ollama", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyOllama {
    pub handle: ProviderHandle,
}

#[pymethods]
impl PyOllama {
    /// Build an Ollama provider.
    ///
    /// `base_url` defaults to `http://localhost:11434` (the
    /// Ollama daemon's standard bind). `timeout_secs` overrides
    /// the 600-second default (local-runner inference can be
    /// slow).
    ///
    /// Phase 28.B / 29.B — `url_prefetch` opts in to tako-side
    /// fetch of `ContentPart::ImageUrl` content (Ollama's
    /// `images: Vec<String>` field accepts only bare base64).
    /// Default is silent-drop. SSRF mitigations baked in:
    /// `https://`-only by default (override via
    /// `url_prefetch_allow_http`); private-IP blocklist on by
    /// default (override via `url_prefetch_allow_private_ips`
    /// for deployments where the network layer already filters
    /// egress); 10s timeout (override via
    /// `url_prefetch_timeout_secs`); 10 MiB cap (override via
    /// `url_prefetch_max_bytes`); MIME validated against
    /// `image/{jpeg,png,gif,webp}`.
    ///
    /// Phase 30.C / 31 — `url_prefetch_allow_hosts` is a per-host
    /// allowlist that bypasses the private-IP blocklist for
    /// specific hostnames only. Phase 31 — entries starting with
    /// `*.` are recognised as wildcard suffix patterns.
    ///
    /// Phase 32.C — `url_prefetch_allow_cidrs` is a CIDR-based
    /// bypass. Pass a list of CIDR strings (`"10.0.5.0/24"`,
    /// `"2001:db8::/32"`); a resolved IP that falls inside any
    /// allowlisted CIDR is permitted. CIDR parse failures
    /// surface from the constructor.
    #[new]
    #[pyo3(signature = (
        model,
        base_url=None,
        timeout_secs=None,
        url_prefetch=false,
        url_prefetch_allow_http=false,
        url_prefetch_allow_private_ips=false,
        url_prefetch_allow_hosts=None,
        url_prefetch_allow_cidrs=None,
        url_prefetch_timeout_secs=None,
        url_prefetch_max_bytes=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        model: String,
        base_url: Option<String>,
        timeout_secs: Option<u64>,
        url_prefetch: bool,
        url_prefetch_allow_http: bool,
        url_prefetch_allow_private_ips: bool,
        url_prefetch_allow_hosts: Option<Vec<String>>,
        url_prefetch_allow_cidrs: Option<Vec<String>>,
        url_prefetch_timeout_secs: Option<u64>,
        url_prefetch_max_bytes: Option<usize>,
    ) -> PyResult<Self> {
        let mut b = OllamaProvider::builder().model(model);
        if let Some(url) = base_url {
            b = b.base_url(url);
        }
        if let Some(secs) = timeout_secs {
            b = b.timeout(std::time::Duration::from_secs(secs));
        }
        if url_prefetch {
            b = b.with_url_prefetch();
        }
        if url_prefetch_allow_http {
            b = b.with_url_prefetch_allow_http();
        }
        if url_prefetch_allow_private_ips {
            b = b.with_url_prefetch_allow_private_ips();
        }
        // Phase 30.C — per-host allowlist.
        if let Some(hosts) = url_prefetch_allow_hosts {
            for host in hosts {
                b = b.with_url_prefetch_allow_host(host);
            }
        }
        // Phase 32.C — CIDR allowlist.
        if let Some(cidrs) = url_prefetch_allow_cidrs {
            for cidr in cidrs {
                b = b.with_url_prefetch_allow_cidr(cidr);
            }
        }
        if let Some(secs) = url_prefetch_timeout_secs {
            b = b.with_url_prefetch_timeout(std::time::Duration::from_secs(secs));
        }
        if let Some(n) = url_prefetch_max_bytes {
            b = b.with_url_prefetch_max_bytes(n);
        }
        let provider = b.build().map_err(map_err)?;
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
