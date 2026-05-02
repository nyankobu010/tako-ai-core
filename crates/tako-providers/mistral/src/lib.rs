//! Mistral La Plateforme chat.completions provider.
//!
//! Implements [`tako_core::LlmProvider`] against
//! `POST https://api.mistral.ai/v1/chat/completions`. Mistral's
//! REST surface is OpenAI-compatible, with two vendor extensions
//! exposed by the builder: `safe_prompt` (server-side safety prefix)
//! and `random_seed` (deterministic sampling).
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use tako_providers_mistral::MistralProvider;
//! let p = MistralProvider::builder()
//!     .api_key_env("MISTRAL_API_KEY")
//!     .model("mistral-large-latest")
//!     .safe_prompt(true)
//!     .build()?;
//! # let _ = p; Ok(()) }
//! ```

mod client;
#[doc(hidden)]
pub mod convert;
#[doc(hidden)]
pub mod stream;

pub use client::{MistralBuilder, MistralProvider};
