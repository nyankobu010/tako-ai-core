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
//! Streaming SSE is documented as Phase 2.5 — the rest of the spec's
//! Phase 2 acceptance ("`tako.compat.serve_openai` passes openai-SDK
//! conformance tests for chat.completions") works without it for the
//! non-streaming path.

mod auth;
mod openai;
mod routes;
mod server;

pub use auth::{AuthResolver, StaticTokens};
pub use server::{ServeConfig, serve_openai};
