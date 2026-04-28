//! Amazon Bedrock provider for tako.
//!
//! Implements [`tako_core::LlmProvider`] against Bedrock's Converse API
//! via the `aws-sdk-bedrockruntime` crate. Streaming
//! (ConverseStream) is documented as a Phase 2.5 follow-up; the
//! single-shot Converse path covers the spec's Phase 2 acceptance test
//! (Bedrock + Converse + tool calls + wiremock-mocked HTTP).
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

pub use client::{BedrockBuilder, BedrockProvider};
