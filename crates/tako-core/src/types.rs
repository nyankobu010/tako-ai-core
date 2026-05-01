//! Core data types shared across `tako`. These are deliberately
//! vendor-agnostic: provider crates convert between vendor-native shapes and
//! these types via their own `convert` modules.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The acting subject of a request — propagated through every layer for
/// multi-tenant isolation, audit, and OPA policy decisions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub tenant_id: String,
    pub user_id: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Free-form metadata; orchestrators use this to track recursion depth,
    /// SelfCaller bookkeeping, etc.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl Principal {
    /// Convenience constructor for tests and single-tenant deployments.
    pub fn anonymous() -> Self {
        Self {
            tenant_id: "anonymous".into(),
            user_id: "anonymous".into(),
            roles: Vec::new(),
            trace_id: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn new(tenant_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            user_id: user_id.into(),
            roles: Vec::new(),
            trace_id: None,
            metadata: BTreeMap::new(),
        }
    }
}

/// Static capability descriptor for a provider/model. Used by `Router`
/// implementations and budget pre-checks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub max_context_tokens: u32,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_json_mode: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_per_input_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_per_output_mtok: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentPart::text(text)],
        }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::text(text)],
        }
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::text(text)],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        mime: String,
        data_b64: String,
    },
    /// Phase 22 — URL-source image. The provider's API server
    /// fetches `url`; tako passes it through unchanged. `mime` is
    /// an optional hint (some vendors use it; others ignore it).
    /// Use for `https://` URLs only — the security story for
    /// `http://` URLs and vendor-specific URI schemes (Vertex's
    /// `gs://...`, Bedrock's pre-fetched bytes, Ollama's
    /// pre-fetched bytes) remains deferred to Phase 23+.
    ///
    /// Wired through Anthropic, OpenAI, and Mistral (the three
    /// vendors whose API servers fetch `https://` URLs directly).
    /// Vertex / Bedrock / Ollama silently drop this variant —
    /// their wire formats either require vendor-specific URIs or
    /// don't support URL sources at all.
    ImageUrl {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        id: String,
        result: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
}

impl ContentPart {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }

    /// Returns the text content if this is a `Text` variant.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text.as_str()),
            _ => None,
        }
    }
}

/// JSON-Schema describing a tool's input. We use `serde_json::Value` rather
/// than a typed schema crate to stay vendor-agnostic; orchestrators MAY
/// validate args against this schema before tool invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

/// MCP 2025-06-18 tool annotations: hints to clients about side-effects.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            stop: Vec::new(),
            stream: false,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    /// Final assistant message — may include tool calls in `content`.
    pub message: Message,
    /// Why the model stopped generating: "stop", "length", "tool_calls",
    /// "content_filter", or a vendor-specific reason.
    pub finish_reason: FinishReason,
    pub usage: Usage,
    /// Vendor-specific extras (e.g. system_fingerprint). Avoid relying on
    /// keys here in cross-provider code paths.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raw: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl Usage {
    pub fn total(&self) -> u32 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// Streaming chunk. The contract is: implementations yield zero or more
/// `Delta` chunks, optionally followed by `Error`, then exactly one `End`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatChunk {
    /// A token / partial content piece for the assistant message.
    Delta {
        /// Text increment, if this delta is text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Streaming tool-call increments. Tool-call streaming is keyed by
        /// `index`; consumers re-assemble incrementally.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCallDelta>,
    },
    /// A non-fatal in-stream error. The stream may still continue.
    Error { message: String },
    /// Final frame; carries the resolved usage and finish reason.
    End {
        finish_reason: FinishReason,
        usage: Usage,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Partial JSON fragment for the args; consumers concat then parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_fragment: Option<String>,
}

/// `Router` decision: which provider/model to dispatch to, with confidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Stable provider id, e.g. `"anthropic:claude-opus-4-7"`.
    pub provider_id: String,
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Snapshot the policy engine sees. Hashes (not raw text) cross the
/// engine boundary so policies don't accidentally read content they aren't
/// supposed to.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyContext {
    pub stage: PolicyStage,
    pub model: String,
    pub messages_hash: String,
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_args_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyStage {
    PreChat,
    PreTool,
    PostChat,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
    RedactMessages { mask: Vec<String> },
    RedactResponse { mask: Vec<String> },
    ForceModel { model: String },
    RequireApproval { reason: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd_per_request: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd_per_day: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_request: Option<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub max_usd_per_tenant_per_day: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BudgetUsage {
    pub usd_this_request: f64,
    pub usd_today: f64,
    pub tokens_this_request: u32,
    pub usd_per_tenant_today: BTreeMap<String, f64>,
}

/// Hint to retry callers: how long the upstream told us to wait.
#[derive(Clone, Copy, Debug)]
pub struct RetryAfter(pub Duration);
