//! `tako-compat` — OpenAI-compatible HTTP server.
//!
//! Wraps any `Orchestrator` (typically a `SingleAgent` or `Conductor`)
//! behind the routes the official `openai` Python SDK speaks:
//!
//! - `POST /v1/chat/completions` — non-streaming chat completion. The
//!   request body is converted to a `ChatRequest`, dispatched through
//!   the orchestrator, and the response shaped back into OpenAI JSON.
//! - `GET  /v1/models` — surfaces the orchestrator's known providers
//!   (today: a single static list since `Orchestrator` doesn't yet
//!   expose its providers; the list comes from the user's serve config).
//! - `GET  /healthz`, `GET  /readyz` — liveness probes.
//!
//! Streaming SSE follows the OpenAI `chat.completion.chunk` shape and
//! terminates with a `data: [DONE]` line, matching the official SDK's
//! parser.

mod auth;
mod openai;
mod routes;
mod server;
mod sse;

#[cfg(feature = "jwt")]
pub use auth::JwtAuthResolver;
#[cfg(feature = "mtls-fs-watch")]
pub use auth::MtlsFsWatcher;
#[cfg(feature = "vault")]
pub use auth::VaultAuthResolver;
#[cfg(feature = "vault")]
pub use auth::{
    AppRoleTokenProvider, DEFAULT_KUBERNETES_JWT_PATH, KubernetesTokenProvider, StaticVaultToken,
    VaultTokenProvider,
};
pub use auth::{AuthResolver, ChainedAuthResolver, ChildShortCircuitPolicy, StaticTokens};
#[cfg(feature = "oidc")]
pub use auth::{IntrospectionAuthMethod, IntrospectionConfig, MtlsClient, OidcAuthResolver};
pub use server::{ServeConfig, serve_openai};
