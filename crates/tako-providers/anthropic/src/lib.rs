//! Anthropic Messages API provider.
//!
//! Implements [`tako_core::LlmProvider`] against `POST /v1/messages` with
//! both single-shot and SSE streaming responses, including incremental
//! tool-use block reassembly.
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use tako_providers_anthropic::AnthropicProvider;
//! let p = AnthropicProvider::builder()
//!     .api_key_env("ANTHROPIC_API_KEY")
//!     .model("claude-opus-4-7")
//!     .build()?;
//! # let _ = p; Ok(()) }
//! ```

mod client;
mod convert;
mod stream;

pub use client::{AnthropicBuilder, AnthropicProvider};
