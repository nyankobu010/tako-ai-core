//! `tako-core` — vendor-neutral traits and types for the tako agentic framework.
//!
//! This crate has **no I/O and no Tokio**. It defines the contracts that
//! provider, transport, orchestrator, and governance crates implement.
//!
//! See the project [README] and [ARCHITECTURE] for context.
//!
//! [README]: https://github.com/TODO(<org>)/tako-ai-core#readme
//! [ARCHITECTURE]: https://github.com/TODO(<org>)/tako-ai-core/blob/main/ARCHITECTURE.md

pub mod error;
pub mod traits;
pub mod types;

pub use error::{ProviderErrorDetails, TakoError};
pub use traits::{
    AlwaysConfident, ConfidenceGuard, ConstantConfidence, LlmProvider, McpTransport, PolicyEngine,
    Router, Tool,
};
pub use types::{
    Budget, BudgetUsage, Capabilities, ChatChunk, ChatRequest, ChatResponse, ContentPart,
    FinishReason, Message, PolicyContext, PolicyDecision, PolicyStage, Principal, RetryAfter, Role,
    RoutingDecision, ToolAnnotations, ToolCallDelta, ToolSchema, Usage,
};
