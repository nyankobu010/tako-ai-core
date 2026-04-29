//! Ollama local-runner provider.
//!
//! Implements [`tako_core::LlmProvider`] against an Ollama server's
//! `POST /api/chat` endpoint. Defaults to `http://localhost:11434`
//! and uses no authentication. Streams responses as newline-delimited
//! JSON (NDJSON), not SSE — see [`stream`] for the framing details.
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use tako_providers_ollama::OllamaProvider;
//! let p = OllamaProvider::builder()
//!     .model("llama3")
//!     .base_url("http://localhost:11434")
//!     .build()?;
//! # let _ = p; Ok(()) }
//! ```

mod client;
#[doc(hidden)]
pub mod convert;
#[doc(hidden)]
pub mod stream;

pub use client::{OllamaBuilder, OllamaProvider};
