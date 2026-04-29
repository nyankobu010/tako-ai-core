//! Google Vertex AI (Gemini) provider.
//!
//! Talks to the Vertex AI REST API (`{location}-aiplatform.googleapis.com`):
//!
//! - non-streaming: `POST /v1/projects/{p}/locations/{l}/publishers/google/models/{m}:generateContent`
//! - streaming:     `POST /v1/projects/{p}/locations/{l}/publishers/google/models/{m}:streamGenerateContent?alt=sse`
//!
//! Authentication is intentionally *deferred to the caller*: the builder
//! accepts a pre-resolved OAuth2 access token (or pulls one from an env
//! var), keeping the `gcp_auth` SDK out of `tako`'s dependency tree.
//! Wire your own credential source in user code:
//!
//! - Quick local dev: `gcloud auth print-access-token`
//! - Service account: any JWT-bearer-grant flow
//! - GKE/Workload Identity: the metadata server
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use tako_providers_vertex::VertexProvider;
//! let p = VertexProvider::builder()
//!     .access_token_env("VERTEX_ACCESS_TOKEN")
//!     .project_id("my-gcp-project")
//!     .model("gemini-2.0-pro")
//!     .build()?;
//! # let _ = p; Ok(()) }
//! ```

mod client;
#[doc(hidden)]
pub mod convert;
#[doc(hidden)]
pub mod stream;

pub use client::{VertexBuilder, VertexProvider};
