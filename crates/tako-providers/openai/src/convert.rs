//! Conversion between `tako-core` types and OpenAI chat.completions JSON.

use serde::{Deserialize, Serialize};
use tako_core::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, TakoError, ToolSchema, Usage,
};

#[derive(Serialize, Debug)]
pub struct OaRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<OaMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OaTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<OaStreamOptions>,
}

#[derive(Serialize, Debug)]
pub struct OaStreamOptions {
    pub include_usage: bool,
}

#[derive(Serialize, Debug)]
pub struct OaTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: OaFunction,
}

#[derive(Serialize, Debug)]
pub struct OaFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Serialize, Debug)]
pub struct OaMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OaToolCall>,
    /// For role=tool, the OpenAI API requires `tool_call_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct OaToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: OaToolCallFunction,
}

#[derive(Serialize, Debug)]
pub struct OaToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Deserialize, Debug)]
pub struct OaResponse {
    pub choices: Vec<OaChoice>,
    #[serde(default)]
    pub usage: OaUsage,
}

#[derive(Deserialize, Debug)]
pub struct OaChoice {
    pub message: OaResponseMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct OaResponseMessage {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<OaResponseToolCall>,
}

#[derive(Deserialize, Debug)]
pub struct OaResponseToolCall {
    pub id: String,
    pub function: OaResponseToolCallFunction,
}

#[derive(Deserialize, Debug)]
pub struct OaResponseToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Deserialize, Debug, Default)]
pub struct OaUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
}

pub fn to_openai_request(req: &ChatRequest) -> OaRequest<'_> {
    OaRequest {
        model: &req.model,
        messages: req.messages.iter().map(message_to_oa).collect(),
        tools: req.tools.iter().map(tool_schema_to_oa).collect(),
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        stop: req.stop.clone(),
        stream: req.stream,
        stream_options: req.stream.then_some(OaStreamOptions { include_usage: true }),
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

fn tool_schema_to_oa(t: &ToolSchema) -> OaTool {
    OaTool {
        kind: "function",
        function: OaFunction {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.input_schema.clone(),
        },
    }
}

fn message_to_oa(m: &Message) -> OaMessage {
    let mut text_parts: Vec<&str> = Vec::new();
    let mut tool_calls: Vec<OaToolCall> = Vec::new();
    let mut tool_call_id: Option<String> = None;
    let mut tool_result_content: Option<String> = None;
    for c in &m.content {
        match c {
            ContentPart::Text { text } => text_parts.push(text),
            ContentPart::ToolCall { id, name, args } => tool_calls.push(OaToolCall {
                id: id.clone(),
                kind: "function",
                function: OaToolCallFunction {
                    name: name.clone(),
                    arguments: args.to_string(),
                },
            }),
            ContentPart::ToolResult { id, result, .. } => {
                tool_call_id = Some(id.clone());
                tool_result_content = Some(result.to_string());
            }
            ContentPart::Image { .. } => {
                // Vision content blocks are out of scope for the Phase-1
                // OpenAI adapter; consumers should drop them or upgrade to
                // a vision-aware provider.
            }
        }
    }
    OaMessage {
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

pub fn from_openai_response(resp: OaResponse) -> Result<ChatResponse, TakoError> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| TakoError::Invalid("openai response had no choices".into()))?;

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
            serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| {
                serde_json::Value::String(tc.function.arguments.clone())
            })
        };
        content.push(ContentPart::ToolCall {
            id: tc.id,
            name: tc.function.name,
            args,
        });
    }

    let finish_reason = match choice.finish_reason.as_deref() {
        Some("stop") => FinishReason::Stop,
        Some("length") => FinishReason::Length,
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
