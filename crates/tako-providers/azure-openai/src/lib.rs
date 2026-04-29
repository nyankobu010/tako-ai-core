//! Azure OpenAI provider.
//!
//! Wire format is identical to OpenAI's chat.completions; the differences are
//! the URL shape and auth header:
//!
//! - URL: `{endpoint}/openai/deployments/{deployment}/chat/completions?api-version={version}`
//! - Auth: `api-key: <key>` header (NOT `Authorization: Bearer`)
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use tako_providers_azure_openai::AzureOpenAiProvider;
//! let p = AzureOpenAiProvider::builder()
//!     .api_key_env("AZURE_OPENAI_API_KEY")
//!     .endpoint("https://my-resource.openai.azure.com")
//!     .deployment("gpt-4o-prod")
//!     .build()?;
//! # let _ = p; Ok(()) }
//! ```

mod client;

pub use client::{AzureOpenAiBuilder, AzureOpenAiProvider};
