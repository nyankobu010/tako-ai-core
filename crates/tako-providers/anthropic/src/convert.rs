//! Conversion between `tako-core` types and Anthropic Messages JSON.
//!
//! Notable differences from OpenAI:
//! - System messages are a top-level `system` field, not in `messages`.
//! - Content blocks live as a typed array (`text`, `tool_use`, `tool_result`).
//! - `tool_result` lives inside a user-role message.

use serde::{Deserialize, Serialize};
use tako_core::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, TakoError, ToolSchema,
    Usage,
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
    /// Phase 19.A — vision content. Anthropic Messages API accepts
    /// images in user-role messages with a base64 source. URL
    /// sources (`source.type = "url"`) need a fetch-and-decode story
    /// we haven't designed yet — Phase 19 emits only base64.
    Image {
        source: AnImageSource,
    },
}

/// Phase 19.A — Anthropic image-source descriptor.
///
/// Only `type = "base64"` is emitted. `media_type` carries the
/// caller-provided MIME (one of `image/jpeg` / `image/png` /
/// `image/gif` / `image/webp` — anything else is dropped before
/// reaching this struct). `data` is raw base64 with any
/// `data:image/...;base64,` data-URL prefix stripped.
#[derive(Serialize, Debug)]
pub struct AnImageSource {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub media_type: String,
    pub data: String,
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
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
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
                let text = m
                    .content
                    .iter()
                    .filter_map(ContentPart::as_text)
                    .collect::<Vec<_>>()
                    .join("");
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
                    messages.push(AnMessage {
                        role,
                        content: blocks,
                    });
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
            ContentPart::Text { text } if !text.is_empty() => {
                Some(AnBlock::Text { text: text.clone() })
            }
            ContentPart::Text { .. } => None,
            ContentPart::ToolCall { id, name, args } => Some(AnBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: args.clone(),
            }),
            ContentPart::ToolResult {
                id,
                result,
                is_error,
            } => Some(AnBlock::ToolResult {
                tool_use_id: id.clone(),
                content: result.to_string(),
                is_error: *is_error,
            }),
            ContentPart::Image { mime, data_b64 } => {
                // Phase 19.A — Anthropic accepts only the four MIME
                // types listed in `is_supported_anthropic_mime`;
                // anything else is silently dropped (matches the
                // empty-text drop above).
                if !is_supported_anthropic_mime(mime) {
                    return None;
                }
                Some(AnBlock::Image {
                    source: AnImageSource {
                        kind: "base64",
                        media_type: mime.clone(),
                        data: strip_data_url_prefix(data_b64).to_string(),
                    },
                })
            }
            // Phase 22.A — placeholder silent-drop. Phase 22.B
            // wires URL-source images through `AnImageSource::Url`
            // (struct → enum refactor in the same commit).
            ContentPart::ImageUrl { .. } => None,
        })
        .collect()
}

/// Phase 19.A — accept only the four MIME types Anthropic's
/// Messages API supports (per public Anthropic docs).
fn is_supported_anthropic_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Phase 19.A — strip a leading `data:image/...;base64,` data-URL
/// prefix when present; return the input unchanged otherwise.
/// Idempotent. Identical canonicalisation to
/// [`tako_providers_bedrock`]'s `data_url_decode`, but here we
/// keep the bytes as base64 (Anthropic accepts only base64-encoded
/// `data` strings, not raw bytes).
fn strip_data_url_prefix(s: &str) -> &str {
    // The data-URL grammar is `data:<mediatype>[;base64],<data>`.
    // We accept any prefix ending in `;base64,` and discard
    // everything up to and including the comma. If the input has
    // no comma, return it unchanged (assume it's already raw
    // base64).
    if let Some(rest) = s.strip_prefix("data:")
        && let Some(comma_at) = rest.find(',')
    {
        &rest[comma_at + 1..]
    } else {
        s
    }
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
                content.push(ContentPart::ToolCall {
                    id,
                    name,
                    args: input,
                });
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------
    // Phase 19.A — outbound image content.
    // -----------------------------------------------------------------

    #[test]
    fn image_block_emits_base64_source() {
        let parts = vec![
            ContentPart::text("describe this"),
            ContentPart::Image {
                mime: "image/png".into(),
                data_b64: "aGVsbG8=".into(),
            },
        ];
        let blocks = content_to_blocks(&parts);
        let serialised = serde_json::to_value(&blocks).unwrap();
        assert_eq!(
            serialised,
            json!([
                { "type": "text", "text": "describe this" },
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "aGVsbG8=",
                    },
                },
            ]),
        );
    }

    #[test]
    fn image_block_strips_data_url_prefix() {
        let parts = vec![ContentPart::Image {
            mime: "image/jpeg".into(),
            data_b64: "data:image/jpeg;base64,YWJjZA==".into(),
        }];
        let blocks = content_to_blocks(&parts);
        // The emitted `data` field must NOT carry the data-URL
        // prefix — Anthropic's API rejects it.
        let serialised = serde_json::to_value(&blocks).unwrap();
        assert_eq!(serialised[0]["source"]["data"], "YWJjZA==",);
    }

    #[test]
    fn image_block_unsupported_mime_dropped() {
        // SVG, BMP, etc. — Anthropic accepts only the four MIME
        // types in `is_supported_anthropic_mime`; anything else is
        // silently dropped to match the empty-text drop policy.
        let parts = vec![
            ContentPart::Image {
                mime: "image/svg+xml".into(),
                data_b64: "<svg/>".into(),
            },
            ContentPart::text("hello"),
        ];
        let blocks = content_to_blocks(&parts);
        // Only the text survives.
        assert_eq!(blocks.len(), 1);
        let serialised = serde_json::to_value(&blocks).unwrap();
        assert_eq!(serialised[0]["type"], "text");
    }

    #[test]
    fn strip_data_url_prefix_idempotent() {
        // Bare base64 (no prefix) should pass through unchanged.
        assert_eq!(strip_data_url_prefix("YWJjZA=="), "YWJjZA==");
        // With prefix.
        assert_eq!(
            strip_data_url_prefix("data:image/png;base64,YWJjZA=="),
            "YWJjZA==",
        );
        // Empty.
        assert_eq!(strip_data_url_prefix(""), "");
    }

    #[test]
    fn supported_mime_smoke() {
        for ok in ["image/jpeg", "image/png", "image/gif", "image/webp"] {
            assert!(is_supported_anthropic_mime(ok), "expected {ok} accepted");
        }
        for bad in ["image/svg+xml", "image/bmp", "text/plain", ""] {
            assert!(!is_supported_anthropic_mime(bad), "expected {bad} rejected");
        }
    }
}
