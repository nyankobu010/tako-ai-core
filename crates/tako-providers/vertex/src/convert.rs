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
    /// Phase 20.A — inline image content. Gemini also accepts
    /// `file_data` for cloud-stored URIs; we don't emit that yet
    /// (server-side fetch from request-supplied URLs has security
    /// implications, same reasoning as Phase 19's `source.type =
    /// "url"` deferral on the Anthropic adapter).
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: VxInlineData,
    },
}

/// Phase 20.A — Gemini `inline_data` payload. `mime_type` is the
/// image MIME (one of `image/jpeg` / `image/png` / `image/gif` /
/// `image/webp`); `data` is raw base64 with any data-URL prefix
/// stripped.
#[derive(Serialize, Debug)]
pub struct VxInlineData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub data: String,
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

    let generation_config =
        if req.temperature.is_some() || req.max_tokens.is_some() || !req.stop.is_empty() {
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
            ContentPart::Image { mime, data_b64 } => {
                // Phase 20.A — Gemini accepts only the four MIME
                // types listed in `is_supported_vertex_mime`;
                // anything else is silently dropped to match the
                // empty-text drop policy elsewhere.
                if !is_supported_vertex_mime(mime) {
                    continue;
                }
                parts.push(VxPart::InlineData {
                    inline_data: VxInlineData {
                        mime_type: mime.clone(),
                        data: strip_data_url_prefix(data_b64).to_string(),
                    },
                });
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

/// Phase 20.A — accept only the four MIME types Gemini's vision
/// surface supports (per Google's published Gemini-pro-vision docs).
fn is_supported_vertex_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Phase 20.A — strip a leading `data:image/...;base64,` data-URL
/// prefix when present; return the input unchanged otherwise.
/// Idempotent. Mirrors the per-crate copies in
/// `tako-providers-anthropic` (Phase 19.A) and
/// `tako-providers-openai` (Phase 19.B) — kept per-crate to
/// preserve provider-crate independence (no cross-provider deps
/// per ARCHITECTURE.md hard rules).
fn strip_data_url_prefix(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("data:")
        && let Some(comma_at) = rest.find(',')
    {
        &rest[comma_at + 1..]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn user_msg(parts: Vec<ContentPart>) -> Message {
        Message {
            role: Role::User,
            content: parts,
        }
    }

    // -----------------------------------------------------------------
    // Phase 20.A — outbound image content for Vertex (Gemini).
    // -----------------------------------------------------------------

    #[test]
    fn image_block_emits_inline_data_part() {
        let m = user_msg(vec![
            ContentPart::text("describe this"),
            ContentPart::Image {
                mime: "image/png".into(),
                data_b64: "aGVsbG8=".into(),
            },
        ]);
        let names: HashMap<&str, &str> = HashMap::new();
        let vx = message_to_vx(&m, &names);
        let serialised = serde_json::to_value(&vx).unwrap();
        // Gemini's REST API uses camelCase fields throughout —
        // matches the existing `functionCall` / `functionResponse`
        // convention in `VxPart`.
        assert_eq!(
            serialised["parts"],
            json!([
                { "text": "describe this" },
                {
                    "inlineData": {
                        "mimeType": "image/png",
                        "data": "aGVsbG8=",
                    },
                },
            ]),
        );
    }

    #[test]
    fn image_block_strips_data_url_prefix() {
        let m = user_msg(vec![ContentPart::Image {
            mime: "image/jpeg".into(),
            data_b64: "data:image/jpeg;base64,YWJjZA==".into(),
        }]);
        let names: HashMap<&str, &str> = HashMap::new();
        let vx = message_to_vx(&m, &names);
        let serialised = serde_json::to_value(&vx).unwrap();
        assert_eq!(serialised["parts"][0]["inlineData"]["data"], "YWJjZA==");
        assert_eq!(
            serialised["parts"][0]["inlineData"]["mimeType"],
            "image/jpeg"
        );
    }

    #[test]
    fn image_block_unsupported_mime_dropped() {
        // SVG / BMP / etc. — silently dropped, matches the
        // empty-text drop policy elsewhere.
        let m = user_msg(vec![
            ContentPart::Image {
                mime: "image/svg+xml".into(),
                data_b64: "<svg/>".into(),
            },
            ContentPart::text("fallback"),
        ]);
        let names: HashMap<&str, &str> = HashMap::new();
        let vx = message_to_vx(&m, &names);
        let serialised = serde_json::to_value(&vx).unwrap();
        // Only the text survives.
        assert_eq!(serialised["parts"], json!([{ "text": "fallback" }]),);
    }

    #[test]
    fn supported_vertex_mime_smoke() {
        for ok in ["image/jpeg", "image/png", "image/gif", "image/webp"] {
            assert!(is_supported_vertex_mime(ok), "expected {ok} accepted");
        }
        for bad in ["image/svg+xml", "image/bmp", "text/plain", ""] {
            assert!(!is_supported_vertex_mime(bad), "expected {bad} rejected");
        }
    }

    #[test]
    fn strip_data_url_prefix_idempotent() {
        assert_eq!(strip_data_url_prefix("YWJjZA=="), "YWJjZA==");
        assert_eq!(
            strip_data_url_prefix("data:image/png;base64,YWJjZA=="),
            "YWJjZA==",
        );
        assert_eq!(strip_data_url_prefix(""), "");
    }
}
