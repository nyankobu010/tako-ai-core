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
    pub content: Option<MiContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<MiToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Phase 20.B — Mistral's vision-capable models (Pixtral) accept
/// `content` as either a flat string OR an array of typed blocks.
/// The array form is required when an `image_url` block is present;
/// otherwise the simpler string form keeps wire shape byte-for-byte
/// compatible with pre-Phase-20 traffic.
#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum MiContent {
    /// Flat string — Phase 1 default. Emitted whenever the message
    /// has no image content parts.
    Text(String),
    /// Array of typed blocks — Phase 20.B. Emitted whenever the
    /// message has at least one [`ContentPart::Image`].
    Blocks(Vec<MiContentBlock>),
}

/// Phase 20.B — Mistral typed content block. Mirrors OpenAI's
/// `{"type": "text", ...}` / `{"type": "image_url", ...}`
/// discriminator (Mistral's vision API is OpenAI-compatible).
#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MiContentBlock {
    Text { text: String },
    ImageUrl { image_url: MiImageUrl },
}

/// Phase 20.B — `image_url` block payload. Mistral accepts both the
/// bare-string `image_url` form and the nested `{"url": "..."}`
/// form; we always emit the nested form for parity with OpenAI's
/// adapter (Phase 19.B).
#[derive(Serialize, Debug)]
pub struct MiImageUrl {
    pub url: String,
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
    let mut tool_calls: Vec<MiToolCall> = Vec::new();
    let mut tool_call_id: Option<String> = None;
    let mut tool_result_content: Option<String> = None;
    // Phase 20.B — track text and image parts in source order so the
    // emitted `content` array preserves narrative order ("here's the
    // question, then here's the image"). When no image parts appear,
    // we fold back to the flat-string form to keep byte-for-byte
    // wire compat with pre-Phase-20 traffic.
    let mut text_parts: Vec<&str> = Vec::new();
    let mut blocks: Vec<MiContentBlock> = Vec::new();
    let mut has_image = false;
    for c in &m.content {
        match c {
            ContentPart::Text { text } => {
                text_parts.push(text);
                blocks.push(MiContentBlock::Text { text: text.clone() });
            }
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
            ContentPart::Image { mime, data_b64 } => {
                // Phase 20.B — accept the four MIME types Mistral's
                // vision endpoint supports (matches OpenAI's set).
                // Anything else is silently dropped to match the
                // empty-text drop policy elsewhere.
                if !is_supported_mistral_mime(mime) {
                    continue;
                }
                has_image = true;
                blocks.push(MiContentBlock::ImageUrl {
                    image_url: MiImageUrl {
                        url: build_data_url(mime, data_b64),
                    },
                });
            }
            // Phase 22.C — URL-source. Mistral's vision API is
            // OpenAI-compatible: pass `url` through to
            // `MiImageUrl.url` unchanged. Same MIME-hint handling
            // as OpenAI (intentionally dropped).
            ContentPart::ImageUrl { url, mime: _ } => {
                has_image = true;
                blocks.push(MiContentBlock::ImageUrl {
                    image_url: MiImageUrl { url: url.clone() },
                });
            }
        }
    }
    let content = if let Some(c) = tool_result_content {
        // Tool-result messages keep the flat-string shape — Mistral's
        // OpenAI-compatible API doesn't accept array content on
        // `role=tool`.
        Some(MiContent::Text(c))
    } else if has_image {
        // Phase 20.B — at least one image: emit the array form.
        Some(MiContent::Blocks(blocks))
    } else if text_parts.is_empty() {
        None
    } else {
        // No images, flat string — byte-for-byte parity with pre-20.B.
        Some(MiContent::Text(text_parts.join("")))
    };
    MiMessage {
        role: role_to_str(m.role),
        content,
        tool_calls,
        tool_call_id,
    }
}

/// Phase 20.B — accept only the four MIME types Mistral's vision
/// endpoint supports (matches OpenAI's set; Mistral's vision API is
/// OpenAI-compatible).
fn is_supported_mistral_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Phase 20.B — build a canonical `data:<mime>;base64,<data>` URL.
/// Mirrors the helper of the same name in `tako-providers-openai`
/// (Phase 19.B). If the caller already supplied a data-URL prefix
/// in `data_b64`, strip it first so we don't end up with a double
/// prefix.
fn build_data_url(mime: &str, data_b64: &str) -> String {
    let raw = strip_data_url_prefix(data_b64);
    format!("data:{mime};base64,{raw}")
}

/// Phase 20.B — strip a leading `data:image/...;base64,` data-URL
/// prefix when present; return the input unchanged otherwise.
/// Idempotent. Per-crate copy of the helper in
/// `tako-providers-{anthropic,openai,vertex}` — kept per-crate to
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

pub fn from_mistral_response(resp: MiResponse) -> Result<ChatResponse, TakoError> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| TakoError::Invalid("mistral response had no choices".into()))?;

    let mut content: Vec<ContentPart> = Vec::new();
    if let Some(text) = choice.message.content
        && !text.is_empty()
    {
        content.push(ContentPart::Text { text });
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use serde_json::json;

    fn user_msg(parts: Vec<ContentPart>) -> Message {
        Message {
            role: Role::User,
            content: parts,
        }
    }

    // -----------------------------------------------------------------
    // Phase 20.B — outbound image content for Mistral.
    // -----------------------------------------------------------------

    #[test]
    fn text_only_message_keeps_flat_string_content() {
        // Regression: pre-20.B wire shape must be preserved
        // byte-for-byte for non-vision messages — `content` is a
        // flat string, not an array.
        let m = user_msg(vec![
            ContentPart::text("hello "),
            ContentPart::text("world"),
        ]);
        let mi = message_to_mi(&m);
        let serialised = serde_json::to_value(&mi).unwrap();
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
        let mi = message_to_mi(&m);
        let serialised = serde_json::to_value(&mi).unwrap();
        // Mistral accepts the OpenAI-compatible nested-`{"url":...}`
        // form; we always emit that for parity with 19.B.
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
        let bare_json = serde_json::to_value(message_to_mi(&bare)).unwrap();
        let prefixed_json = serde_json::to_value(message_to_mi(&prefixed)).unwrap();
        assert_eq!(bare_json, prefixed_json);
        assert_eq!(
            bare_json["content"][0]["image_url"]["url"],
            "data:image/jpeg;base64,YWJjZA==",
        );
    }

    #[test]
    fn image_block_unsupported_mime_dropped() {
        let m = user_msg(vec![
            ContentPart::Image {
                mime: "image/svg+xml".into(),
                data_b64: "<svg/>".into(),
            },
            ContentPart::text("fallback"),
        ]);
        let mi = message_to_mi(&m);
        let serialised = serde_json::to_value(&mi).unwrap();
        // No image part survives → no array shape; text falls through
        // to the flat-string form.
        assert_eq!(serialised["content"], "fallback");
    }

    #[test]
    fn tool_result_message_keeps_flat_string_content() {
        // Tool-result messages must keep the flat-string shape —
        // Mistral's OpenAI-compatible API doesn't accept array
        // content on role=tool.
        let m = Message {
            role: Role::Tool,
            content: vec![ContentPart::ToolResult {
                id: "call_123".into(),
                result: json!({"ok": true}),
                is_error: false,
            }],
        };
        let mi = message_to_mi(&m);
        let serialised = serde_json::to_value(&mi).unwrap();
        assert_eq!(serialised["content"], "{\"ok\":true}");
        assert_eq!(serialised["tool_call_id"], "call_123");
    }

    #[test]
    fn supported_mistral_mime_smoke() {
        for ok in ["image/jpeg", "image/png", "image/gif", "image/webp"] {
            assert!(is_supported_mistral_mime(ok));
        }
        for bad in ["image/svg+xml", "image/bmp", "text/plain"] {
            assert!(!is_supported_mistral_mime(bad));
        }
    }

    // -----------------------------------------------------------------
    // Phase 22.C — URL-source images via `MiImageUrl.url`.
    // -----------------------------------------------------------------

    #[test]
    fn image_url_block_emits_array_content_with_url() {
        let m = user_msg(vec![
            ContentPart::text("describe this"),
            ContentPart::ImageUrl {
                url: "https://example.com/cat.jpg".into(),
                mime: None,
            },
        ]);
        let mi = message_to_mi(&m);
        let serialised = serde_json::to_value(&mi).unwrap();
        assert_eq!(
            serialised["content"],
            json!([
                { "type": "text", "text": "describe this" },
                {
                    "type": "image_url",
                    "image_url": { "url": "https://example.com/cat.jpg" },
                },
            ]),
        );
    }

    #[test]
    fn image_url_does_not_get_data_url_wrapped() {
        // Regression pin: URL passes through verbatim. Wrapping
        // `https://...` in `data:image/...;base64,` would break it.
        let m = user_msg(vec![ContentPart::ImageUrl {
            url: "https://example.com/dog.png".into(),
            mime: Some("image/png".into()),
        }]);
        let mi = message_to_mi(&m);
        let serialised = serde_json::to_value(&mi).unwrap();
        let url = serialised["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert_eq!(url, "https://example.com/dog.png");
        assert!(!url.starts_with("data:"), "got: {url}");
    }
}
