//! Fallback chain: cascade through providers on transient errors.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tako_core::{Capabilities, ChatChunk, ChatRequest, ChatResponse, LlmProvider, Principal, TakoError};

/// Wraps a primary [`LlmProvider`] with an ordered list of fallbacks. On a
/// transient error from the primary, the call cascades through the fallback
/// list in order. Non-transient errors fail fast.
///
/// `id()` and `capabilities()` reflect the primary; the fallbacks may have
/// different capabilities, but consumers see the primary's contract.
pub struct FallbackProvider {
    primary: Arc<dyn LlmProvider>,
    fallbacks: Vec<Arc<dyn LlmProvider>>,
}

impl std::fmt::Debug for FallbackProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackProvider")
            .field("primary", &self.primary.id())
            .field("fallbacks", &self.fallbacks.iter().map(|p| p.id()).collect::<Vec<_>>())
            .finish()
    }
}

impl FallbackProvider {
    pub fn new(primary: Arc<dyn LlmProvider>, fallbacks: Vec<Arc<dyn LlmProvider>>) -> Self {
        Self { primary, fallbacks }
    }
}

#[async_trait]
impl LlmProvider for FallbackProvider {
    fn id(&self) -> &str {
        self.primary.id()
    }

    fn capabilities(&self) -> &Capabilities {
        self.primary.capabilities()
    }

    async fn chat(&self, principal: &Principal, req: ChatRequest) -> Result<ChatResponse, TakoError> {
        let mut last_err = match self.primary.chat(principal, req.clone()).await {
            Ok(r) => return Ok(r),
            Err(e) if !e.is_transient() => return Err(e),
            Err(e) => e,
        };
        for fb in &self.fallbacks {
            tracing::warn!(primary = %self.primary.id(), fallback = %fb.id(), reason = %last_err, "falling back");
            match fb.chat(principal, req.clone()).await {
                Ok(r) => return Ok(r),
                Err(e) if !e.is_transient() => return Err(e),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    async fn stream(
        &self,
        principal: &Principal,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        // Streaming fallback only fires on the *initial* connect error;
        // partial-stream failures cannot be retried because chunks have
        // already been delivered to the consumer.
        let mut last_err = match self.primary.stream(principal, req.clone()).await {
            Ok(s) => return Ok(s),
            Err(e) if !e.is_transient() => return Err(e),
            Err(e) => e,
        };
        for fb in &self.fallbacks {
            tracing::warn!(primary = %self.primary.id(), fallback = %fb.id(), reason = %last_err, "falling back (stream)");
            match fb.stream(principal, req.clone()).await {
                Ok(s) => return Ok(s),
                Err(e) if !e.is_transient() => return Err(e),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    fn estimate_cost_usd(&self, req: &ChatRequest) -> f64 {
        self.primary.estimate_cost_usd(req)
    }
}
