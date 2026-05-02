//! Amazon Bedrock provider for tako.
//!
//! Implements [`tako_core::LlmProvider`] against Bedrock's Converse +
//! ConverseStream APIs via the `aws-sdk-bedrockruntime` crate. Supports
//! text, tool calls, tool results, and streaming.
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use tako_providers_bedrock::BedrockProvider;
//! let p = BedrockProvider::builder()
//!     .model("anthropic.claude-3-5-sonnet-20240620-v1:0")
//!     .build().await?;
//! # let _ = p; Ok(()) }
//! ```

mod client;
mod convert;
mod stream;
mod url_prefetch;

pub use client::{BedrockBuilder, BedrockProvider};
