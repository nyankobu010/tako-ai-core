//! Conversion between `tako-core` types and Vertex AI Gemini JSON.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tako_core::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, TakoError, ToolSchema,
    Usage,
};

// ---------- Request ----------

#[derive(Serialize, Debug)]
pub struct VxRequest {
    pub contents: Vec<VxContent>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemInstruction")]
    pub system_instruction: Option<VxContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<VxTool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "generationConfig")]
    pub generation_config: Option<VxGenerationConfig>,
}

#[derive(Serialize, Debug)]
pub struct VxContent {
    /// Vertex roles: "user" | "model" | "function". Tool-result messages
    /// become "function" with a `functionResponse` part. System instructions
    /// are sent in a separate top-level `systemInstruction` field instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    pub parts: Vec<VxPart>,
}

#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum VxPart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: VxFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: VxFunctionResponse,
    },
}

#[derive(Serialize, Debug)]
pub struct VxFunctionCall {
    pub name: String,
    pub args: Value,
}

#[derive(Serialize, Debug)]
pub struct VxFunctionResponse {
    pub name: String,
    pub response: Value,
}

#[derive(Serialize, Debug)]
pub struct VxTool {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<VxFunctionDeclaration>,
}

#[derive(Serialize, Debug)]
pub struct VxFunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Serialize, Debug, Default)]
pub struct VxGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxOutputTokens")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", rename = "stopSequences")]
    pub stop_sequences: Vec<String>,
}

pub fn to_vertex_request(req: &ChatRequest) -> VxRequest {
    let mut contents = Vec::new();
    let mut system_text_parts: Vec<String> = Vec::new();

    // Vertex's `functionResponse` schema requires the function name, but
    // tako's `ContentPart::ToolResult` only carries the call id. Walk the
    // conversation once to resolve id -> name from prior tool calls.
    let mut tool_call_names: std::collections::HashMap<&str, &str> =
        std::collections::HashMap::new();
    for m in &req.messages {
        for c in &m.content {
            if let ContentPart::ToolCall { id, name, .. } = c {
                tool_call_names.insert(id.as_str(), name.as_str());
            }
        }
    }

    for m in &req.messages {
        if matches!(m.role, Role::System) {
            // Vertex doesn't have an in-line system role; collapse all
            // system text into the top-level `systemInstruction` field.
            for c in &m.content {
                if let ContentPart::Text { text } = c {
                    system_text_parts.push(text.clone());
                }
            }
            continue;
        }
        contents.push(message_to_vx(m, &tool_call_names));
    }

    let system_instruction = (!system_text_parts.is_empty()).then(|| VxContent {
        role: None,
        parts: vec![VxPart::Text {
            text: system_text_parts.join("\n"),
        }],
    });

    let tools = if req.tools.is_empty() {
        Vec::new()
    } else {
        vec![VxTool {
            function_declarations: req.tools.iter().map(tool_schema_to_vx).collect(),
        }]
    };

    let generation_config = if req.temperature.is_some()
        || req.max_tokens.is_some()
        || !req.stop.is_empty()
    {
        Some(VxGenerationConfig {
            temperature: req.temperature,
            max_output_tokens: req.max_tokens,
            stop_sequences: req.stop.clone(),
        })
    } else {
        None
    };

    VxRequest {
        contents,
        system_instruction,
        tools,
        generation_config,
    }
}

fn role_to_vx(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "model",
        Role::Tool => "function",
        Role::System => "user", // unreachable here; system is hoisted
    }
}

fn tool_schema_to_vx(t: &ToolSchema) -> VxFunctionDeclaration {
    VxFunctionDeclaration {
        name: t.name.clone(),
        description: t.description.clone(),
        parameters: t.input_schema.clone(),
    }
}

fn message_to_vx(
    m: &Message,
    tool_call_names: &std::collections::HashMap<&str, &str>,
) -> VxContent {
    let mut parts: Vec<VxPart> = Vec::new();
    for c in &m.content {
        match c {
            ContentPart::Text { text } => {
                if !text.is_empty() {
                    parts.push(VxPart::Text { text: text.clone() });
                }
            }
            ContentPart::ToolCall { name, args, .. } => parts.push(VxPart::FunctionCall {
                function_call: VxFunctionCall {
                    name: name.clone(),
                    args: args.clone(),
                },
            }),
            ContentPart::ToolResult { id, result, .. } => {
                // If we can't find the original call's name (orchestrator
                // sent a ToolResult without a corresponding prior ToolCall
                // in the same request), fall back to the id — wire schema
                // requires *some* name field; Vertex will surface a clearer
                // error than us silently dropping the part.
                let name = tool_call_names
                    .get(id.as_str())
                    .copied()
                    .unwrap_or(id.as_str())
                    .to_string();
                parts.push(VxPart::FunctionResponse {
                    function_response: VxFunctionResponse {
                        name,
                        response: result.clone(),
                    },
                })
            }
            ContentPart::Image { .. } => {
                // Vision parts deferred; drop silently for now.
            }
        }
    }
    VxContent {
        role: Some(role_to_vx(m.role)),
        parts,
    }
}

// ---------- Response ----------

#[derive(Deserialize, Debug)]
pub struct VxResponse {
    #[serde(default)]
    pub candidates: Vec<VxCandidate>,
    #[serde(default, rename = "usageMetadata")]
    pub usage_metadata: Option<VxUsage>,
}

#[derive(Deserialize, Debug)]
pub struct VxCandidate {
    #[serde(default)]
    pub content: Option<VxResponseContent>,
    #[serde(default, rename = "finishReason")]
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct VxResponseContent {
    #[serde(default)]
    pub parts: Vec<VxResponsePart>,
}

#[derive(Deserialize, Debug)]
pub struct VxResponsePart {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, rename = "functionCall")]
    pub function_call: Option<VxResponseFunctionCall>,
}

#[derive(Deserialize, Debug)]
pub struct VxResponseFunctionCall {
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Deserialize, Debug, Default, Clone, Copy)]
pub struct VxUsage {
    #[serde(default, rename = "promptTokenCount")]
    pub prompt_tokens: u32,
    #[serde(default, rename = "candidatesTokenCount")]
    pub output_tokens: u32,
}

pub fn finish_reason_from_vx(s: Option<&str>) -> FinishReason {
    match s {
        Some("STOP") => FinishReason::Stop,
        Some("MAX_TOKENS") => FinishReason::Length,
        Some("SAFETY") | Some("RECITATION") | Some("BLOCKLIST") | Some("PROHIBITED_CONTENT") => {
            FinishReason::ContentFilter
        }
        _ => FinishReason::Other,
    }
}

pub fn from_vertex_response(resp: VxResponse) -> Result<ChatResponse, TakoError> {
    let candidate = resp
        .candidates
        .into_iter()
        .next()
        .ok_or_else(|| TakoError::Invalid("vertex response had no candidates".into()))?;

    let mut content: Vec<ContentPart> = Vec::new();
    let mut had_tool_call = false;

    if let Some(c) = candidate.content {
        for part in c.parts {
            if let Some(text) = part.text {
                if !text.is_empty() {
                    content.push(ContentPart::Text { text });
                }
            }
            if let Some(fc) = part.function_call {
                had_tool_call = true;
                // Vertex tool calls have no stable id; synthesise a
                // deterministic-per-call placeholder so the orchestrator
                // can correlate with later tool results.
                content.push(ContentPart::ToolCall {
                    id: format!("vertex_call_{}", content.len()),
                    name: fc.name,
                    args: fc.args,
                });
            }
        }
    }

    let raw_finish = candidate.finish_reason.as_deref();
    let mut finish_reason = finish_reason_from_vx(raw_finish);
    if had_tool_call && matches!(finish_reason, FinishReason::Stop | FinishReason::Other) {
        finish_reason = FinishReason::ToolCalls;
    }

    let usage = resp.usage_metadata.unwrap_or_default();

    Ok(ChatResponse {
        message: Message {
            role: Role::Assistant,
            content,
        },
        finish_reason,
        usage: Usage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.output_tokens,
        },
        raw: Default::default(),
    })
}
