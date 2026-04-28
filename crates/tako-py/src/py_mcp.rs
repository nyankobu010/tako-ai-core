//! `#[pyclass]`es for MCP transports: `PyStdio` and `PyStreamableHttp`.
//!
//! Both expose a stable Python surface that the orchestrator picks up via a
//! shared [`McpTransportHandle`].

use std::sync::Arc;
use std::time::Duration;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tako_core::McpTransport;
use tako_mcp::{StdioTransport, StreamableHttpTransport};

use crate::py_provider::map_err;

/// Shared, cloneable handle the orchestrator extracts from any transport
/// `#[pyclass]`. Holds an `Arc<dyn McpTransport>` so the same transport
/// can be referenced from both Python and Rust.
#[derive(Clone)]
pub struct McpTransportHandle {
    pub inner: Arc<dyn McpTransport>,
}

impl std::fmt::Debug for McpTransportHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTransportHandle").finish_non_exhaustive()
    }
}

#[pyclass(name = "Stdio", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyStdio {
    pub handle: McpTransportHandle,
    pub command: String,
}

#[pymethods]
impl PyStdio {
    /// Spawn an MCP server subprocess and complete the
    /// `initialize` → `initialized` handshake. Blocks the calling Python
    /// thread until the handshake returns; the GIL is released for the
    /// blocking section so other threads can progress.
    #[new]
    #[pyo3(signature = (command, args=None))]
    fn new(py: Python<'_>, command: String, args: Option<Vec<String>>) -> PyResult<Self> {
        let args = args.unwrap_or_default();
        let cmd = command.clone();
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let transport = py.detach(|| {
            rt.block_on(async {
                let t = StdioTransport::spawn(&cmd, &args).await?;
                let arc: Arc<dyn McpTransport> = Arc::new(t);
                tako_mcp::handshake(Arc::clone(&arc), tako_mcp::ClientInfo::tako()).await?;
                Ok::<_, tako_core::TakoError>(arc)
            })
        });
        let inner = transport.map_err(map_err)?;
        Ok(Self {
            handle: McpTransportHandle { inner },
            command,
        })
    }

    fn __repr__(&self) -> String {
        format!("Stdio(command={:?})", self.command)
    }
}

#[pyclass(name = "StreamableHttp", module = "tako._native", from_py_object)]
#[derive(Clone, Debug)]
pub struct PyStreamableHttp {
    pub handle: McpTransportHandle,
    pub url: String,
}

#[pymethods]
impl PyStreamableHttp {
    /// Build a Streamable-HTTP MCP transport pointing at `url`.
    /// `headers` is an optional list of `(key, value)` tuples; values may
    /// reference environment variables via `$ENV_VAR`.
    #[new]
    #[pyo3(signature = (url, headers=None, timeout_secs=None))]
    fn new(
        py: Python<'_>,
        url: String,
        headers: Option<Vec<(String, String)>>,
        timeout_secs: Option<u64>,
    ) -> PyResult<Self> {
        let mut b = StreamableHttpTransport::builder().url(&url);
        for (k, v) in headers.unwrap_or_default() {
            b = b.header(k, v);
        }
        if let Some(t) = timeout_secs {
            b = b.timeout(Duration::from_secs(t));
        }
        let transport = b.build().map_err(map_err)?;
        let arc: Arc<dyn McpTransport> = Arc::new(transport);
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        // Run the lifecycle handshake; HTTP servers may or may not require
        // it depending on their capabilities, so failures here are fatal
        // (better to surface now than silently in the first tools/list).
        let result: Result<(), tako_core::TakoError> = py.detach(|| {
            rt.block_on(async {
                tako_mcp::handshake(Arc::clone(&arc), tako_mcp::ClientInfo::tako()).await?;
                Ok(())
            })
        });
        result.map_err(map_err)?;
        Ok(Self {
            handle: McpTransportHandle { inner: arc },
            url,
        })
    }

    fn __repr__(&self) -> String {
        format!("StreamableHttp(url={:?})", self.url)
    }
}

/// Try to extract an [`McpTransportHandle`] from any of the transport
/// `#[pyclass]`es. Used by the orchestrator's constructor to accept a
/// heterogeneous `mcp_servers=[...]` argument.
pub fn extract_transport_handle(py: Python<'_>, obj: &Py<PyAny>) -> PyResult<McpTransportHandle> {
    if let Ok(t) = obj.extract::<PyStdio>(py) {
        return Ok(t.handle);
    }
    if let Ok(t) = obj.extract::<PyStreamableHttp>(py) {
        return Ok(t.handle);
    }
    Err(PyValueError::new_err(
        "mcp_servers entries must be tako._native.Stdio or tako._native.StreamableHttp instances",
    ))
}
