//! OpenAI chat.completions provider.
//!
//! Implements [`tako_core::LlmProvider`] against `POST /v1/chat/completions`
//! with both single-shot and SSE streaming responses, including incremental
//! tool-call delta reassembly.
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use tako_providers_openai::OpenAiProvider;
//! let p = OpenAiProvider::builder()
//!     .api_key_env("OPENAI_API_KEY")
//!     .model("gpt-5")
//!     .build()?;
//! # let _ = p; Ok(()) }
//! ```

mod client;
mod convert;
mod stream;

pub use client::{OpenAiBuilder, OpenAiProvider};
