//! Conversion between `tako-core` types and Anthropic Messages JSON.
//!
//! Notable differences from OpenAI:
//! - System messages are a top-level `system` field, not in `messages`.
//! - Content blocks live as a typed array (`text`, `tool_use`, `tool_result`).
//! - `tool_result` lives inside a user-role message.

use serde::{Deserialize, Serialize};
use tako_core::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, TakoError, ToolSchema, Usage,
};

#[derive(Serialize, Debug)]
pub struct AnRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<AnMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AnTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
}

#[derive(Serialize, Debug)]
pub struct AnMessage {
    pub role: &'static str,
    pub content: Vec<AnBlock>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Serialize, Debug)]
pub struct AnTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub struct AnResponse {
    #[serde(default)]
    pub content: Vec<AnResponseBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: AnUsage,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnResponseBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
}

#[derive(Deserialize, Debug, Default)]
pub struct AnUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

pub fn to_anthropic_request<'a>(req: &'a ChatRequest, default_max_tokens: u32) -> AnRequest<'a> {
    let mut system: Option<String> = None;
    let mut messages: Vec<AnMessage> = Vec::new();
    for m in &req.messages {
        match m.role {
            Role::System => {
                let text = m.content.iter().filter_map(ContentPart::as_text).collect::<Vec<_>>().join("");
                if !text.is_empty() {
                    system = Some(text);
                }
            }
            other => {
                let role = match other {
                    Role::User | Role::Tool => "user",
                    Role::Assistant => "assistant",
                    Role::System => unreachable!(),
                };
                let blocks = content_to_blocks(&m.content);
                if !blocks.is_empty() {
                    messages.push(AnMessage { role, content: blocks });
                }
            }
        }
    }

    let max_tokens = req.max_tokens.unwrap_or(default_max_tokens);

    AnRequest {
        model: &req.model,
        messages,
        system,
        max_tokens,
        tools: req.tools.iter().map(tool_schema_to_an).collect(),
        temperature: req.temperature,
        stop_sequences: req.stop.clone(),
        stream: req.stream,
    }
}

fn tool_schema_to_an(t: &ToolSchema) -> AnTool {
    AnTool {
        name: t.name.clone(),
        description: t.description.clone(),
        input_schema: t.input_schema.clone(),
    }
}

fn content_to_blocks(parts: &[ContentPart]) -> Vec<AnBlock> {
    parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text { text } if !text.is_empty() => Some(AnBlock::Text { text: text.clone() }),
            ContentPart::Text { .. } => None,
            ContentPart::ToolCall { id, name, args } => Some(AnBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: args.clone(),
            }),
            ContentPart::ToolResult { id, result, is_error } => Some(AnBlock::ToolResult {
                tool_use_id: id.clone(),
                content: result.to_string(),
                is_error: *is_error,
            }),
            ContentPart::Image { .. } => None, // vision is out of scope for Phase 1
        })
        .collect()
}

pub fn from_anthropic_response(resp: AnResponse) -> Result<ChatResponse, TakoError> {
    let mut content: Vec<ContentPart> = Vec::new();
    for block in resp.content {
        match block {
            AnResponseBlock::Text { text } if !text.is_empty() => {
                content.push(ContentPart::Text { text });
            }
            AnResponseBlock::Text { .. } => {}
            AnResponseBlock::ToolUse { id, name, input } => {
                content.push(ContentPart::ToolCall { id, name, args: input });
            }
        }
    }

    let finish_reason = match resp.stop_reason.as_deref() {
        Some("end_turn") => FinishReason::Stop,
        Some("max_tokens") => FinishReason::Length,
        Some("tool_use") => FinishReason::ToolCalls,
        Some("stop_sequence") => FinishReason::Stop,
        _ => FinishReason::Other,
    };

    Ok(ChatResponse {
        message: Message {
            role: Role::Assistant,
            content,
        },
        finish_reason,
        usage: Usage {
            input_tokens: resp.usage.input_tokens,
            output_tokens: resp.usage.output_tokens,
        },
        raw: Default::default(),
    })
}
