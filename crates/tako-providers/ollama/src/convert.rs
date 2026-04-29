//! Conversion between `tako-core` types and Ollama `/api/chat` JSON.
//!
//! Ollama's chat surface is *similar* to OpenAI's but differs in three
//! ways the adapter must handle:
//! - `tool_calls[].function.arguments` is a JSON object (not a string).
//! - There is no top-level `choices` array — the message is at the top.
//! - Token counts arrive as `prompt_eval_count` / `eval_count`.

use serde::{Deserialize, Serialize};
use tako_core::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, TakoError, ToolSchema,
    Usage,
};

#[derive(Serialize, Debug)]
pub struct OlRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<OlMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OlTool>,
    pub stream: bool,
    #[serde(skip_serializing_if = "OlOptions::is_empty")]
    pub options: OlOptions,
}

#[derive(Serialize, Debug, Default)]
pub struct OlOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

impl OlOptions {
    fn is_empty(&self) -> bool {
        self.temperature.is_none() && self.num_predict.is_none() && self.stop.is_empty()
    }
}

#[derive(Serialize, Debug)]
pub struct OlTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: OlFunction,
}

#[derive(Serialize, Debug)]
pub struct OlFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Serialize, Debug)]
pub struct OlMessage {
    pub role: &'static str,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OlToolCall>,
}

#[derive(Serialize, Debug)]
pub struct OlToolCall {
    pub function: OlToolCallFunction,
}

#[derive(Serialize, Debug)]
pub struct OlToolCallFunction {
    pub name: String,
    /// Ollama accepts either a string or a JSON object here. We serialise
    /// as an object to match the response shape.
    pub arguments: serde_json::Value,
}

#[derive(Deserialize, Debug, Default)]
pub struct OlResponse {
    #[serde(default)]
    pub message: OlResponseMessage,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
    #[serde(default)]
    pub prompt_eval_count: u32,
    #[serde(default)]
    pub eval_count: u32,
}

#[derive(Deserialize, Debug, Default)]
pub struct OlResponseMessage {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<OlResponseToolCall>,
}

#[derive(Deserialize, Debug)]
pub struct OlResponseToolCall {
    pub function: OlResponseToolCallFunction,
}

#[derive(Deserialize, Debug)]
pub struct OlResponseToolCallFunction {
    pub name: String,
    /// Ollama emits arguments as a structured JSON object.
    pub arguments: serde_json::Value,
}

pub fn to_ollama_request(req: &ChatRequest) -> OlRequest<'_> {
    OlRequest {
        model: &req.model,
        messages: req.messages.iter().map(message_to_ol).collect(),
        tools: req.tools.iter().map(tool_schema_to_ol).collect(),
        stream: req.stream,
        options: OlOptions {
            temperature: req.temperature,
            num_predict: req.max_tokens,
            stop: req.stop.clone(),
        },
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

fn tool_schema_to_ol(t: &ToolSchema) -> OlTool {
    OlTool {
        kind: "function",
        function: OlFunction {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.input_schema.clone(),
        },
    }
}

fn message_to_ol(m: &Message) -> OlMessage {
    let mut text_parts: Vec<&str> = Vec::new();
    let mut tool_calls: Vec<OlToolCall> = Vec::new();
    let mut tool_result: Option<String> = None;
    for c in &m.content {
        match c {
            ContentPart::Text { text } => text_parts.push(text),
            ContentPart::ToolCall { name, args, .. } => tool_calls.push(OlToolCall {
                function: OlToolCallFunction {
                    name: name.clone(),
                    arguments: args.clone(),
                },
            }),
            ContentPart::ToolResult { result, .. } => {
                tool_result = Some(result.to_string());
            }
            ContentPart::Image { .. } => {
                // Vision is out of scope for this adapter.
            }
        }
    }
    OlMessage {
        role: role_to_str(m.role),
        content: tool_result.unwrap_or_else(|| text_parts.join("")),
        tool_calls,
    }
}

pub fn from_ollama_response(resp: OlResponse) -> Result<ChatResponse, TakoError> {
    let mut content: Vec<ContentPart> = Vec::new();
    if !resp.message.content.is_empty() {
        content.push(ContentPart::Text {
            text: resp.message.content,
        });
    }
    let had_tool_calls = !resp.message.tool_calls.is_empty();
    for (i, tc) in resp.message.tool_calls.into_iter().enumerate() {
        content.push(ContentPart::ToolCall {
            // Ollama doesn't issue tool-call ids; synthesise one so
            // downstream `ToolResult { id }` correlation works.
            id: format!("ol_call_{i}"),
            name: tc.function.name,
            args: tc.function.arguments,
        });
    }

    // Tool calls take precedence over the textual `done_reason`:
    // Ollama reports "stop" even when the model emitted tool calls and
    // is implicitly waiting for the caller to provide tool results.
    let finish_reason = if had_tool_calls {
        FinishReason::ToolCalls
    } else {
        match resp.done_reason.as_deref() {
            Some("length") => FinishReason::Length,
            _ => FinishReason::Stop,
        }
    };

    Ok(ChatResponse {
        message: Message {
            role: Role::Assistant,
            content,
        },
        finish_reason,
        usage: Usage {
            input_tokens: resp.prompt_eval_count,
            output_tokens: resp.eval_count,
        },
        raw: Default::default(),
    })
}
