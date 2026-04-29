//! Conversion between `tako-core` types and Mistral chat.completions JSON.
//!
//! Mistral La Plateforme is OpenAI-compatible, so the wire format mirrors
//! the OpenAI adapter byte-for-byte. The two vendor extensions we expose
//! are `safe_prompt` and `random_seed`.

use serde::{Deserialize, Serialize};
use tako_core::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, TakoError, ToolSchema,
    Usage,
};

#[derive(Serialize, Debug)]
pub struct MiRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<MiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<MiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    /// Mistral-specific: server-side safety preamble.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_prompt: Option<bool>,
    /// Mistral-specific: deterministic sampling seed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub random_seed: Option<u64>,
}

#[derive(Serialize, Debug)]
pub struct MiTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: MiFunction,
}

#[derive(Serialize, Debug)]
pub struct MiFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Serialize, Debug)]
pub struct MiMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<MiToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct MiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: MiToolCallFunction,
}

#[derive(Serialize, Debug)]
pub struct MiToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Deserialize, Debug)]
pub struct MiResponse {
    pub choices: Vec<MiChoice>,
    #[serde(default)]
    pub usage: MiUsage,
}

#[derive(Deserialize, Debug)]
pub struct MiChoice {
    pub message: MiResponseMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct MiResponseMessage {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<MiResponseToolCall>,
}

#[derive(Deserialize, Debug)]
pub struct MiResponseToolCall {
    pub id: String,
    pub function: MiResponseToolCallFunction,
}

#[derive(Deserialize, Debug)]
pub struct MiResponseToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Deserialize, Debug, Default)]
pub struct MiUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
}

pub fn to_mistral_request<'a>(
    req: &'a ChatRequest,
    safe_prompt: Option<bool>,
    random_seed: Option<u64>,
) -> MiRequest<'a> {
    MiRequest {
        model: &req.model,
        messages: req.messages.iter().map(message_to_mi).collect(),
        tools: req.tools.iter().map(tool_schema_to_mi).collect(),
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        stop: req.stop.clone(),
        stream: req.stream,
        safe_prompt,
        random_seed,
    }
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn tool_schema_to_mi(t: &ToolSchema) -> MiTool {
    MiTool {
        kind: "function",
        function: MiFunction {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.input_schema.clone(),
        },
    }
}

fn message_to_mi(m: &Message) -> MiMessage {
    let mut text_parts: Vec<&str> = Vec::new();
    let mut tool_calls: Vec<MiToolCall> = Vec::new();
    let mut tool_call_id: Option<String> = None;
    let mut tool_result_content: Option<String> = None;
    for c in &m.content {
        match c {
            ContentPart::Text { text } => text_parts.push(text),
            ContentPart::ToolCall { id, name, args } => tool_calls.push(MiToolCall {
                id: id.clone(),
                kind: "function",
                function: MiToolCallFunction {
                    name: name.clone(),
                    arguments: args.to_string(),
                },
            }),
            ContentPart::ToolResult { id, result, .. } => {
                tool_call_id = Some(id.clone());
                tool_result_content = Some(result.to_string());
            }
            ContentPart::Image { .. } => {
                // Vision is out of scope for the Mistral adapter.
            }
        }
    }
    MiMessage {
        role: role_to_str(m.role),
        content: if let Some(c) = tool_result_content {
            Some(c)
        } else if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        },
        tool_calls,
        tool_call_id,
    }
}

pub fn from_mistral_response(resp: MiResponse) -> Result<ChatResponse, TakoError> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| TakoError::Invalid("mistral response had no choices".into()))?;

    let mut content: Vec<ContentPart> = Vec::new();
    if let Some(text) = choice.message.content {
        if !text.is_empty() {
            content.push(ContentPart::Text { text });
        }
    }
    for tc in choice.message.tool_calls {
        let args: serde_json::Value = if tc.function.arguments.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&tc.function.arguments)
                .unwrap_or_else(|_| serde_json::Value::String(tc.function.arguments.clone()))
        };
        content.push(ContentPart::ToolCall {
            id: tc.id,
            name: tc.function.name,
            args,
        });
    }

    let finish_reason = match choice.finish_reason.as_deref() {
        Some("stop") => FinishReason::Stop,
        Some("length") | Some("model_length") => FinishReason::Length,
        Some("tool_calls") | Some("function_call") => FinishReason::ToolCalls,
        Some("content_filter") => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    };

    Ok(ChatResponse {
        message: Message {
            role: Role::Assistant,
            content,
        },
        finish_reason,
        usage: Usage {
            input_tokens: resp.usage.prompt_tokens,
            output_tokens: resp.usage.completion_tokens,
        },
        raw: Default::default(),
    })
}
