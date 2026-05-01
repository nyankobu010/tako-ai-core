//! Conversion between `tako-core` types and OpenAI chat.completions JSON.

use serde::{Deserialize, Serialize};
use tako_core::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, TakoError, ToolSchema,
    Usage,
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
    pub content: Option<OaContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OaToolCall>,
    /// For role=tool, the OpenAI API requires `tool_call_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Phase 19.B — OpenAI Chat Completions accepts `content` as either
/// a flat string OR an array of typed blocks. Array form is required
/// when an `image_url` block is present; otherwise the simpler
/// string form keeps wire shape byte-for-byte compatible with
/// pre-Phase-19 traffic.
#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum OaContent {
    /// Flat string — Phase 1 default. Emitted whenever the message
    /// has no image content parts.
    Text(String),
    /// Array of typed blocks — Phase 19.B. Emitted whenever the
    /// message has at least one [`ContentPart::Image`].
    Blocks(Vec<OaContentBlock>),
}

/// Phase 19.B — OpenAI typed content block. Mirrors the
/// `{"type": "text", ...}` / `{"type": "image_url", ...}` discriminator
/// in OpenAI's Chat Completions content-array format.
#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OaContentBlock {
    Text { text: String },
    ImageUrl { image_url: OaImageUrl },
}

/// Phase 19.B — `image_url` block payload. The `url` field carries
/// either an `https://...` URL or a `data:image/...;base64,...`
/// data-URL. Phase 19 emits only data-URLs; remote URLs are deferred
/// (server-side fetch from request-supplied URLs has security
/// implications).
#[derive(Serialize, Debug)]
pub struct OaImageUrl {
    pub url: String,
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
        stream_options: req.stream.then_some(OaStreamOptions {
            include_usage: true,
        }),
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
    let mut tool_calls: Vec<OaToolCall> = Vec::new();
    let mut tool_call_id: Option<String> = None;
    let mut tool_result_content: Option<String> = None;
    // Phase 19.B — track text and image parts in source order so the
    // emitted `content` array preserves narrative order ("here's the
    // question, then here's the image"). When no image parts appear,
    // we fold back to the flat-string form to keep byte-for-byte
    // wire compat with pre-Phase-19 traffic.
    let mut text_parts: Vec<&str> = Vec::new();
    let mut blocks: Vec<OaContentBlock> = Vec::new();
    let mut has_image = false;
    for c in &m.content {
        match c {
            ContentPart::Text { text } => {
                text_parts.push(text);
                blocks.push(OaContentBlock::Text { text: text.clone() });
            }
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
            ContentPart::Image { mime, data_b64 } => {
                // Phase 19.B — accept the four MIME types OpenAI's
                // vision endpoint supports. Anything else is silently
                // dropped to match the empty-text drop policy elsewhere.
                if !is_supported_openai_mime(mime) {
                    continue;
                }
                has_image = true;
                blocks.push(OaContentBlock::ImageUrl {
                    image_url: OaImageUrl {
                        url: build_data_url(mime, data_b64),
                    },
                });
            }
            // Phase 22.A — placeholder silent-drop. Phase 22.C
            // wires URL-source images by passing `url` straight
            // through to `OaImageUrl.url` (no `data:` prefix
            // wrapping).
            ContentPart::ImageUrl { .. } => {}
        }
    }
    let content = if let Some(c) = tool_result_content {
        // Tool-result messages keep the flat-string shape — the
        // OpenAI API doesn't accept array content on `role=tool`.
        Some(OaContent::Text(c))
    } else if has_image {
        // Phase 19.B — at least one image: emit the array form.
        // `blocks` already carries text+image entries in source
        // order.
        Some(OaContent::Blocks(blocks))
    } else if text_parts.is_empty() {
        None
    } else {
        // No images, flat string — byte-for-byte parity with pre-19.B.
        Some(OaContent::Text(text_parts.join("")))
    };
    OaMessage {
        role: role_to_str(m.role),
        content,
        tool_calls,
        tool_call_id,
    }
}

/// Phase 19.B — accept only the four MIME types OpenAI's vision
/// endpoint supports (per the Chat Completions docs).
fn is_supported_openai_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Phase 19.B — build a canonical `data:<mime>;base64,<data>` URL.
/// If the caller already supplied a data-URL prefix in `data_b64`,
/// strip it first so we don't end up with a double prefix.
fn build_data_url(mime: &str, data_b64: &str) -> String {
    let raw = strip_data_url_prefix(data_b64);
    format!("data:{mime};base64,{raw}")
}

/// Phase 19.B — strip a leading `data:image/...;base64,` data-URL
/// prefix when present; return the input unchanged otherwise.
/// Idempotent. Mirrors the helper of the same name in
/// `tako-providers-anthropic`.
fn strip_data_url_prefix(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("data:")
        && let Some(comma_at) = rest.find(',')
    {
        &rest[comma_at + 1..]
    } else {
        s
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use serde_json::json;
    use tako_core::Role;

    fn user_msg(parts: Vec<ContentPart>) -> Message {
        Message {
            role: Role::User,
            content: parts,
        }
    }

    // -----------------------------------------------------------------
    // Phase 19.B — outbound image content.
    // -----------------------------------------------------------------

    #[test]
    fn text_only_message_keeps_flat_string_content() {
        // Regression: pre-19.B wire shape must be preserved
        // byte-for-byte for non-vision messages — `content` is a
        // flat string, not an array.
        let m = user_msg(vec![
            ContentPart::text("hello "),
            ContentPart::text("world"),
        ]);
        let oa = message_to_oa(&m);
        let serialised = serde_json::to_value(&oa).unwrap();
        assert_eq!(serialised["content"], "hello world");
    }

    #[test]
    fn image_block_emits_array_content_with_image_url() {
        let m = user_msg(vec![
            ContentPart::text("describe this"),
            ContentPart::Image {
                mime: "image/png".into(),
                data_b64: "aGVsbG8=".into(),
            },
        ]);
        let oa = message_to_oa(&m);
        let serialised = serde_json::to_value(&oa).unwrap();
        // Array shape preserves source order: text then image.
        assert_eq!(
            serialised["content"],
            json!([
                { "type": "text", "text": "describe this" },
                {
                    "type": "image_url",
                    "image_url": { "url": "data:image/png;base64,aGVsbG8=" },
                },
            ]),
        );
    }

    #[test]
    fn image_block_normalises_data_url_prefix() {
        // Bare base64 + prefixed input must yield the same canonical
        // data-URL shape in the request.
        let bare = user_msg(vec![ContentPart::Image {
            mime: "image/jpeg".into(),
            data_b64: "YWJjZA==".into(),
        }]);
        let prefixed = user_msg(vec![ContentPart::Image {
            mime: "image/jpeg".into(),
            data_b64: "data:image/jpeg;base64,YWJjZA==".into(),
        }]);
        let bare_json = serde_json::to_value(message_to_oa(&bare)).unwrap();
        let prefixed_json = serde_json::to_value(message_to_oa(&prefixed)).unwrap();
        assert_eq!(bare_json, prefixed_json);
        assert_eq!(
            bare_json["content"][0]["image_url"]["url"],
            "data:image/jpeg;base64,YWJjZA==",
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
        let oa = message_to_oa(&m);
        let serialised = serde_json::to_value(&oa).unwrap();
        // No image part → no array shape; text falls through to the
        // flat-string form.
        assert_eq!(serialised["content"], "fallback");
    }

    #[test]
    fn tool_result_message_keeps_flat_string_content() {
        // Tool-result messages must keep the flat-string shape —
        // OpenAI's API doesn't accept array content on role=tool.
        let m = Message {
            role: Role::Tool,
            content: vec![ContentPart::ToolResult {
                id: "call_123".into(),
                result: json!({"ok": true}),
                is_error: false,
            }],
        };
        let oa = message_to_oa(&m);
        let serialised = serde_json::to_value(&oa).unwrap();
        // Content is the JSON-stringified result, NOT an array.
        assert_eq!(serialised["content"], "{\"ok\":true}");
        assert_eq!(serialised["tool_call_id"], "call_123");
    }

    #[test]
    fn supported_openai_mime_smoke() {
        for ok in ["image/jpeg", "image/png", "image/gif", "image/webp"] {
            assert!(is_supported_openai_mime(ok));
        }
        for bad in ["image/svg+xml", "image/bmp", "text/plain"] {
            assert!(!is_supported_openai_mime(bad));
        }
    }

    #[test]
    fn build_data_url_idempotent_on_prefix() {
        assert_eq!(
            build_data_url("image/png", "YWI="),
            "data:image/png;base64,YWI=",
        );
        assert_eq!(
            build_data_url("image/png", "data:image/jpeg;base64,YWI="),
            "data:image/png;base64,YWI=",
        );
    }
}
