//! OpenAI wire-format types for the compat server's `/v1/chat/completions`
//! endpoint. Mirrors what the official `openai` Python SDK sends and
//! expects.

use serde::{Deserialize, Serialize};
use tako_core::{ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, Usage};

#[derive(Debug, Deserialize)]
pub struct OaChatRequest {
    pub model: String,
    pub messages: Vec<OaMessage>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stop: Option<OaStop>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum OaStop {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
pub struct OaMessage {
    pub role: String,
    pub content: Option<String>,
    /// For role=tool — accepted on input so the OpenAI SDK can replay
    /// a tool-result message; we discard it on the server side because
    /// orchestrator-side tool routing already runs through tako-mcp's
    /// ToolRegistry.
    #[serde(default)]
    #[allow(dead_code)]
    pub tool_call_id: Option<String>,
}

pub fn from_openai_request(req: OaChatRequest) -> ChatRequest {
    let messages = req
        .messages
        .into_iter()
        .map(|m| {
            let role = match m.role.as_str() {
                "system" => Role::System,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            let content = match m.content {
                Some(text) if !text.is_empty() => vec![ContentPart::Text { text }],
                _ => Vec::new(),
            };
            Message { role, content }
        })
        .collect();

    let stop = match req.stop {
        Some(OaStop::One(s)) => vec![s],
        Some(OaStop::Many(v)) => v,
        None => Vec::new(),
    };

    ChatRequest {
        model: req.model,
        messages,
        tools: Vec::new(),
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        stop,
        stream: req.stream,
        metadata: Default::default(),
    }
}

#[derive(Debug, Serialize)]
pub struct OaResponse {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OaChoice>,
    pub usage: OaUsage,
}

#[derive(Debug, Serialize)]
pub struct OaChoice {
    pub index: u32,
    pub message: OaResponseMessage,
    pub finish_reason: &'static str,
}

#[derive(Debug, Serialize)]
pub struct OaResponseMessage {
    pub role: &'static str,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct OaUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

pub fn to_openai_response(model: String, resp: ChatResponse) -> OaResponse {
    let text = resp
        .message
        .content
        .iter()
        .filter_map(ContentPart::as_text)
        .collect::<Vec<_>>()
        .join("");
    OaResponse {
        id: format!("chatcmpl-tako-{}", id_blob()),
        object: "chat.completion",
        created: now_secs(),
        model,
        choices: vec![OaChoice {
            index: 0,
            message: OaResponseMessage {
                role: "assistant",
                content: text,
            },
            finish_reason: finish_reason_str(resp.finish_reason),
        }],
        usage: usage_to_oa(resp.usage),
    }
}

fn finish_reason_str(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Error => "error",
        FinishReason::Other => "stop",
    }
}

fn usage_to_oa(u: Usage) -> OaUsage {
    let total = u.input_tokens.saturating_add(u.output_tokens);
    OaUsage {
        prompt_tokens: u.input_tokens,
        completion_tokens: u.output_tokens,
        total_tokens: total,
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn id_blob() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[derive(Debug, Serialize)]
pub struct OaModelsList {
    pub object: &'static str,
    pub data: Vec<OaModel>,
}

#[derive(Debug, Serialize)]
pub struct OaModel {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: &'static str,
}

pub fn models_list(models: &[String]) -> OaModelsList {
    let now = now_secs();
    OaModelsList {
        object: "list",
        data: models
            .iter()
            .map(|id| OaModel {
                id: id.clone(),
                object: "model",
                created: now,
                owned_by: "tako",
            })
            .collect(),
    }
}
