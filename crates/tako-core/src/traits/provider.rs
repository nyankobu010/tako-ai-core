//! `LlmProvider` — the vendor-neutral chat-completion contract.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::TakoError;
use crate::types::{Capabilities, ChatChunk, ChatRequest, ChatResponse, Principal};

/// A vendor-neutral chat-completion provider.
///
/// Implementations exist in the `tako-providers/*` crates. New providers can
/// be added in pure Rust or, for prototyping, via the `PythonProvider` shim
/// in `tako-py`.
///
/// # Example
///
/// ```no_run
/// use std::sync::Arc;
/// use tako_core::{LlmProvider, ChatRequest, Principal, Message};
/// # async fn run(p: Arc<dyn LlmProvider>) -> Result<(), Box<dyn std::error::Error>> {
/// let req = ChatRequest::new("model", vec![Message::user("hello")]);
/// let resp = p.chat(&Principal::anonymous(), req).await?;
/// println!("finish_reason = {:?}", resp.finish_reason);
/// # Ok(()) }
/// ```
#[async_trait]
pub trait LlmProvider: Send + Sync + 'static {
    /// Stable identifier, e.g. `"anthropic:claude-opus-4-7"`.
    fn id(&self) -> &str;

    /// Static capabilities for the provider/model pair.
    fn capabilities(&self) -> &Capabilities;

    /// Single-shot chat completion.
    async fn chat(&self, principal: &Principal, req: ChatRequest) -> Result<ChatResponse, TakoError>;

    /// Streaming variant. Implementations MUST yield `ChatChunk::End`
    /// exactly once (including on error after partial output).
    async fn stream(
        &self,
        principal: &Principal,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError>;

    /// Approximate cost estimator used by `Budget` enforcement BEFORE the
    /// call. Implementations should be fast and pessimistic. Default: 0.
    fn estimate_cost_usd(&self, _req: &ChatRequest) -> f64 {
        0.0
    }
}
