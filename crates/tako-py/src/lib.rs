//! `tako-py` — PyO3 bindings, exposed as the `tako._native` extension module.
//!
//! GIL discipline (per spec §12, enforced in code review):
//!
//! 1. The shared Tokio runtime comes from
//!    [`pyo3_async_runtimes::tokio::get_runtime`]; we never build a second.
//! 2. Async wrappers convert futures via
//!    [`pyo3_async_runtimes::tokio::future_into_py`].
//! 3. Sync siblings (`*_sync`) call `py.detach(|| rt.block_on(...))` so
//!    other Python threads can take the GIL while we wait.
//! 4. Inside futures we never call `Python::attach` while holding I/O.

#![allow(unsafe_code)] // PyO3 macro expansions emit unsafe extern blocks; required for FFI.

mod conv;
mod py_ab_mcts;
mod py_azure;
mod py_bedrock;
mod py_compat;
mod py_conductor;
mod py_governance;
mod py_http_generic;
mod py_mcp;
mod py_ollama;
mod py_orch_event;
mod py_orch_output;
mod py_orchestrator;
mod py_provider;
mod py_python_provider;
mod py_router;
mod py_runtime;
mod py_secrets;
mod py_self_caller;
#[cfg(feature = "sigstore")]
mod py_sigstore;
mod py_trinity;
mod py_vertex;

use pyo3::prelude::*;

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<py_provider::PyOpenAI>()?;
    m.add_class::<py_provider::PyAnthropic>()?;
    m.add_class::<py_provider::PyFakeProvider>()?;
    m.add_class::<py_azure::PyAzureOpenAi>()?;
    m.add_class::<py_bedrock::PyBedrock>()?;
    m.add_class::<py_ollama::PyOllama>()?;
    m.add_class::<py_vertex::PyVertex>()?;
    m.add_class::<py_http_generic::PyHttpGeneric>()?;
    m.add_class::<py_python_provider::PyPythonProvider>()?;
    m.add_class::<py_orchestrator::PyOrchestrator>()?;
    m.add_class::<py_conductor::PyConductor>()?;
    m.add_class::<py_trinity::PyTrinity>()?;
    m.add_class::<py_self_caller::PySelfCaller>()?;
    m.add_class::<py_self_caller::PyRuleBasedGuard>()?;
    m.add_class::<py_self_caller::PyLlmJudgeGuard>()?;
    m.add_class::<py_ab_mcts::PyAbMcts>()?;
    m.add_class::<py_ab_mcts::PyRuleBasedVerifier>()?;
    m.add_class::<py_orch_event::PyOrchEvent>()?;
    m.add_class::<py_orch_event::PyOrchEventStream>()?;
    m.add_class::<py_orch_output::PyOrchOutput>()?;
    m.add_class::<py_router::PyRegexRouter>()?;
    #[cfg(feature = "onnx")]
    m.add_class::<py_router::PyOnnxRouter>()?;
    m.add_class::<py_mcp::PyStdio>()?;
    m.add_class::<py_mcp::PyStreamableHttp>()?;
    #[cfg(feature = "ws")]
    m.add_class::<py_mcp::PyWebSocket>()?;
    #[cfg(feature = "grpc")]
    m.add_class::<py_mcp::PyGrpc>()?;
    m.add_class::<py_governance::PyBudget>()?;
    #[cfg(feature = "sigstore")]
    m.add_class::<py_sigstore::PyCatalogueVerifier>()?;
    #[cfg(feature = "sigstore")]
    m.add_class::<py_sigstore::PyKeylessVerifier>()?;
    #[cfg(feature = "sigstore")]
    m.add_class::<py_sigstore::PyTrustRoot>()?;
    #[cfg(feature = "sigstore")]
    m.add_class::<py_sigstore::PyJsonStateStore>()?;
    #[cfg(feature = "redis")]
    m.add_class::<py_sigstore::PyRedisStateStore>()?;
    m.add_class::<py_runtime::PyInMemoryBudgetBackend>()?;
    #[cfg(feature = "redis")]
    m.add_class::<py_runtime::PyRedisBudgetBackend>()?;
    m.add_class::<py_secrets::PyVaultResolver>()?;
    m.add_class::<py_secrets::PyAzureKeyVaultResolver>()?;
    m.add_class::<py_secrets::PyGcpSecretManagerResolver>()?;
    m.add_class::<py_secrets::PyAwsSecretsManagerResolver>()?;
    m.add_function(wrap_pyfunction!(py_governance::init_tracing_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_governance::init_otlp_tracing_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_governance::shutdown_otlp_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_compat::serve_openai_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_compat::shutdown_compat_py, m)?)?;
    #[cfg(feature = "auth-jwt")]
    m.add_class::<py_compat::PyJwtAuth>()?;
    #[cfg(feature = "auth-oidc")]
    m.add_class::<py_compat::PyOidcAuth>()?;
    #[cfg(feature = "auth-oidc")]
    m.add_class::<py_compat::PyMtlsRefreshHook>()?;
    #[cfg(feature = "auth-mtls-fs-watch")]
    m.add_class::<py_compat::PyMtlsFsWatcher>()?;
    #[cfg(feature = "auth-mtls-identity-provider")]
    m.add_class::<py_compat::PyMtlsIdentityProvider>()?;
    #[cfg(feature = "auth-mtls-identity-provider")]
    m.add_class::<py_compat::PyMtlsProviderWatcher>()?;
    #[cfg(feature = "auth-vault")]
    m.add_class::<py_compat::PyVaultAuth>()?;
    // Phase 21.B — composite resolver. Always-on (no feature gate);
    // children themselves carry whatever `auth-*` gates they were
    // built under.
    m.add_class::<py_compat::PyChainedAuth>()?;
    m.add_function(wrap_pyfunction!(featurise_text_py, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}

/// Expose the Rust featuriser to Python so the training harness's
/// parity test can compare both sides byte-for-byte.
#[pyfunction]
#[pyo3(name = "featurise_text")]
fn featurise_text_py(text: &str) -> Vec<f32> {
    tako_orchestrator::featurise_text(text)
}
