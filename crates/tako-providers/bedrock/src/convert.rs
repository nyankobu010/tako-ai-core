//! Conversion between `tako-core` types and the Bedrock Converse SDK types.

use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::types as br;
use aws_smithy_types::{Document, Number};
use tako_core::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Role, TakoError, ToolSchema,
    Usage,
};

/// Build the inputs to a Bedrock `Converse` request:
/// `(messages, system, inference_config, tool_config)`.
pub struct ConverseInputs {
    pub messages: Vec<br::Message>,
    pub system: Vec<br::SystemContentBlock>,
    pub inference_config: Option<br::InferenceConfiguration>,
    pub tool_config: Option<br::ToolConfiguration>,
}

pub fn to_converse_inputs(req: &ChatRequest) -> Result<ConverseInputs, TakoError> {
    let mut system: Vec<br::SystemContentBlock> = Vec::new();
    let mut messages: Vec<br::Message> = Vec::new();
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
                    system.push(br::SystemContentBlock::Text(text));
                }
            }
            other => {
                let role = match other {
                    Role::User | Role::Tool => br::ConversationRole::User,
                    Role::Assistant => br::ConversationRole::Assistant,
                    Role::System => unreachable!(),
                };
                let blocks = content_to_blocks(&m.content)?;
                if !blocks.is_empty() {
                    let msg = br::Message::builder()
                        .role(role)
                        .set_content(Some(blocks))
                        .build()
                        .map_err(|e| TakoError::Invalid(format!("Bedrock message build: {e}")))?;
                    messages.push(msg);
                }
            }
        }
    }

    let inference_config =
        if req.temperature.is_some() || req.max_tokens.is_some() || !req.stop.is_empty() {
            let mut cfg = br::InferenceConfiguration::builder();
            if let Some(t) = req.temperature {
                cfg = cfg.temperature(t);
            }
            if let Some(m) = req.max_tokens {
                cfg = cfg.max_tokens(i32::try_from(m).unwrap_or(i32::MAX));
            }
            if !req.stop.is_empty() {
                cfg = cfg.set_stop_sequences(Some(req.stop.clone()));
            }
            Some(cfg.build())
        } else {
            None
        };

    let tool_config = if req.tools.is_empty() {
        None
    } else {
        Some(build_tool_config(&req.tools)?)
    };

    Ok(ConverseInputs {
        messages,
        system,
        inference_config,
        tool_config,
    })
}

fn build_tool_config(tools: &[ToolSchema]) -> Result<br::ToolConfiguration, TakoError> {
    let tool_specs: Vec<br::Tool> = tools
        .iter()
        .map(|t| {
            let schema_doc = json_to_document(&t.input_schema);
            let spec = br::ToolSpecification::builder()
                .name(&t.name)
                .description(&t.description)
                .input_schema(br::ToolInputSchema::Json(schema_doc))
                .build()
                .map_err(|e| TakoError::Invalid(format!("Bedrock tool spec build: {e}")))?;
            Ok(br::Tool::ToolSpec(spec))
        })
        .collect::<Result<_, TakoError>>()?;
    br::ToolConfiguration::builder()
        .set_tools(Some(tool_specs))
        .build()
        .map_err(|e| TakoError::Invalid(format!("Bedrock tool_config build: {e}")))
}

/// Recursively convert a `serde_json::Value` into an `aws_smithy_types::Document`.
fn json_to_document(value: &serde_json::Value) -> Document {
    match value {
        serde_json::Value::Null => Document::Null,
        serde_json::Value::Bool(b) => Document::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Document::Number(Number::NegInt(i))
            } else if let Some(u) = n.as_u64() {
                Document::Number(Number::PosInt(u))
            } else if let Some(f) = n.as_f64() {
                Document::Number(Number::Float(f))
            } else {
                Document::Null
            }
        }
        serde_json::Value::String(s) => Document::String(s.clone()),
        serde_json::Value::Array(arr) => {
            Document::Array(arr.iter().map(json_to_document).collect())
        }
        serde_json::Value::Object(map) => Document::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_document(v)))
                .collect(),
        ),
    }
}

/// Recursively convert an `aws_smithy_types::Document` back to JSON.
fn document_to_json(doc: &Document) -> serde_json::Value {
    use serde_json::Value;
    match doc {
        Document::Null => Value::Null,
        Document::Bool(b) => Value::Bool(*b),
        Document::Number(n) => match n {
            Number::PosInt(u) => Value::Number((*u).into()),
            Number::NegInt(i) => Value::Number((*i).into()),
            Number::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        },
        Document::String(s) => Value::String(s.clone()),
        Document::Array(arr) => Value::Array(arr.iter().map(document_to_json).collect()),
        Document::Object(map) => {
            let mut obj = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                obj.insert(k.clone(), document_to_json(v));
            }
            Value::Object(obj)
        }
    }
}

fn content_to_blocks(parts: &[ContentPart]) -> Result<Vec<br::ContentBlock>, TakoError> {
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        match p {
            ContentPart::Text { text } if !text.is_empty() => {
                out.push(br::ContentBlock::Text(text.clone()));
            }
            ContentPart::Text { .. } => {}
            ContentPart::ToolCall { id, name, args } => {
                let input_doc = json_to_document(args);
                let block = br::ToolUseBlock::builder()
                    .tool_use_id(id)
                    .name(name)
                    .input(input_doc)
                    .build()
                    .map_err(|e| TakoError::Invalid(format!("Bedrock ToolUseBlock build: {e}")))?;
                out.push(br::ContentBlock::ToolUse(block));
            }
            ContentPart::ToolResult {
                id,
                result,
                is_error,
            } => {
                let inner = br::ToolResultContentBlock::Text(result.to_string());
                let block = br::ToolResultBlock::builder()
                    .tool_use_id(id)
                    .content(inner)
                    .status(if *is_error {
                        br::ToolResultStatus::Error
                    } else {
                        br::ToolResultStatus::Success
                    })
                    .build()
                    .map_err(|e| {
                        TakoError::Invalid(format!("Bedrock ToolResultBlock build: {e}"))
                    })?;
                out.push(br::ContentBlock::ToolResult(block));
            }
            ContentPart::Image { mime, data_b64 } => {
                let bytes = data_url_decode(data_b64);
                let format = match mime.as_str() {
                    "image/png" => br::ImageFormat::Png,
                    "image/jpeg" => br::ImageFormat::Jpeg,
                    "image/gif" => br::ImageFormat::Gif,
                    "image/webp" => br::ImageFormat::Webp,
                    _ => continue,
                };
                let source = br::ImageSource::Bytes(Blob::new(bytes));
                let block = br::ImageBlock::builder()
                    .format(format)
                    .source(source)
                    .build()
                    .map_err(|e| TakoError::Invalid(format!("Bedrock ImageBlock build: {e}")))?;
                out.push(br::ContentBlock::Image(block));
            }
            // Phase 22.A — silent-drop. The AWS Bedrock SDK's
            // `ImageSource` exposes only `Bytes`; there's no URL
            // variant, so URL-source images would require a
            // tako-side pre-fetch (back to the SSRF concern that
            // Phase 22 explicitly dodged by doing vendor-fetched
            // URLs only). Deferred to Phase 23+.
            ContentPart::ImageUrl { .. } => {}
        }
    }
    Ok(out)
}

fn data_url_decode(data_b64: &str) -> Vec<u8> {
    // Strip a "data:...;base64," prefix if present, then decode via
    // aws-smithy-types (already pulled in transitively, no extra dep).
    let raw = data_b64.split(',').next_back().unwrap_or(data_b64);
    aws_smithy_types::base64::decode(raw).unwrap_or_default()
}

pub fn from_converse_output(
    output: aws_sdk_bedrockruntime::operation::converse::ConverseOutput,
) -> Result<ChatResponse, TakoError> {
    let stop_reason = match output.stop_reason() {
        br::StopReason::EndTurn => FinishReason::Stop,
        br::StopReason::StopSequence => FinishReason::Stop,
        br::StopReason::MaxTokens | br::StopReason::ModelContextWindowExceeded => {
            FinishReason::Length
        }
        br::StopReason::ToolUse => FinishReason::ToolCalls,
        br::StopReason::ContentFiltered => FinishReason::ContentFilter,
        br::StopReason::GuardrailIntervened => FinishReason::ContentFilter,
        br::StopReason::MalformedModelOutput | br::StopReason::MalformedToolUse => {
            FinishReason::Error
        }
        _ => FinishReason::Other,
    };

    let usage = output
        .usage()
        .map(|u| Usage {
            input_tokens: u32::try_from(u.input_tokens()).unwrap_or(0),
            output_tokens: u32::try_from(u.output_tokens()).unwrap_or(0),
        })
        .unwrap_or_default();

    let mut content: Vec<ContentPart> = Vec::new();
    if let Some(br::ConverseOutput::Message(msg)) = output.output {
        for block in msg.content {
            match block {
                br::ContentBlock::Text(text) if !text.is_empty() => {
                    content.push(ContentPart::Text { text });
                }
                br::ContentBlock::Text(_) => {}
                br::ContentBlock::ToolUse(tu) => {
                    let args = document_to_json(&tu.input);
                    content.push(ContentPart::ToolCall {
                        id: tu.tool_use_id,
                        name: tu.name,
                        args,
                    });
                }
                _ => { /* ignore image / reasoning / etc. for Phase 2 minimal scope */ }
            }
        }
    }

    Ok(ChatResponse {
        message: Message {
            role: Role::Assistant,
            content,
        },
        finish_reason: stop_reason,
        usage,
        raw: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use aws_sdk_bedrockruntime::operation::converse::ConverseOutput;
    use serde_json::json;

    #[test]
    fn json_document_round_trip() {
        let value = json!({
            "name": "tako",
            "size": 42,
            "ratio": 1.5,
            "active": true,
            "tags": ["a", "b"],
            "missing": null,
        });
        let doc = json_to_document(&value);
        let back = document_to_json(&doc);
        assert_eq!(back, value);
    }

    #[test]
    fn to_converse_inputs_extracts_system() {
        let req = ChatRequest {
            model: "anthropic.claude".into(),
            messages: vec![Message::system("You are helpful."), Message::user("hi")],
            tools: vec![],
            temperature: None,
            max_tokens: None,
            stop: vec![],
            stream: false,
            metadata: Default::default(),
        };
        let inputs = to_converse_inputs(&req).unwrap();
        assert_eq!(inputs.system.len(), 1);
        match &inputs.system[0] {
            br::SystemContentBlock::Text(s) => assert_eq!(s, "You are helpful."),
            _ => panic!("expected text system block"),
        }
        assert_eq!(inputs.messages.len(), 1);
    }

    #[test]
    fn to_converse_inputs_builds_tool_config() {
        let req = ChatRequest {
            model: "anthropic.claude".into(),
            messages: vec![Message::user("call search")],
            tools: vec![ToolSchema {
                name: "search".into(),
                description: "Search the index.".into(),
                input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}),
                annotations: None,
            }],
            temperature: Some(0.4),
            max_tokens: Some(2048),
            stop: vec!["\n\n".into()],
            stream: false,
            metadata: Default::default(),
        };
        let inputs = to_converse_inputs(&req).unwrap();
        assert!(inputs.tool_config.is_some(), "tool_config not built");
        assert!(
            inputs.inference_config.is_some(),
            "inference_config not built"
        );
    }

    #[test]
    fn to_converse_inputs_threads_tool_call_and_result() {
        let req = ChatRequest {
            model: "anthropic.claude".into(),
            messages: vec![
                Message::user("search"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentPart::ToolCall {
                        id: "toolu_1".into(),
                        name: "search".into(),
                        args: json!({"q": "tako"}),
                    }],
                },
                Message {
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        id: "toolu_1".into(),
                        result: json!({"hits": 3}),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![],
            temperature: None,
            max_tokens: None,
            stop: vec![],
            stream: false,
            metadata: Default::default(),
        };
        let inputs = to_converse_inputs(&req).unwrap();
        // user, assistant (tool_use), user (tool_result wrapped as user-role)
        assert_eq!(inputs.messages.len(), 3);
        let last = &inputs.messages[2];
        assert_eq!(*last.role(), br::ConversationRole::User);
        assert!(matches!(last.content()[0], br::ContentBlock::ToolResult(_)));
    }

    #[test]
    fn from_converse_output_text_and_tool_use() {
        let assistant = br::Message::builder()
            .role(br::ConversationRole::Assistant)
            .content(br::ContentBlock::Text("hello".into()))
            .content(br::ContentBlock::ToolUse(
                br::ToolUseBlock::builder()
                    .tool_use_id("toolu_1")
                    .name("search")
                    .input(Document::Object(
                        [("q".to_string(), Document::String("tako".into()))]
                            .into_iter()
                            .collect(),
                    ))
                    .build()
                    .unwrap(),
            ))
            .build()
            .unwrap();
        let output = ConverseOutput::builder()
            .output(br::ConverseOutput::Message(assistant))
            .stop_reason(br::StopReason::ToolUse)
            .usage(
                br::TokenUsage::builder()
                    .input_tokens(5)
                    .output_tokens(8)
                    .total_tokens(13)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let resp = from_converse_output(output).unwrap();
        assert_eq!(resp.finish_reason, FinishReason::ToolCalls);
        assert_eq!(resp.usage.input_tokens, 5);
        assert_eq!(resp.usage.output_tokens, 8);
        // First content is text, second is tool call.
        assert_eq!(resp.message.content[0].as_text(), Some("hello"));
        match &resp.message.content[1] {
            ContentPart::ToolCall { id, name, args } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "search");
                assert_eq!(args["q"], "tako");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn from_converse_output_max_tokens_maps_to_length() {
        let assistant = br::Message::builder()
            .role(br::ConversationRole::Assistant)
            .content(br::ContentBlock::Text("partial".into()))
            .build()
            .unwrap();
        let output = ConverseOutput::builder()
            .output(br::ConverseOutput::Message(assistant))
            .stop_reason(br::StopReason::MaxTokens)
            .usage(
                br::TokenUsage::builder()
                    .input_tokens(1)
                    .output_tokens(1)
                    .total_tokens(2)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let resp = from_converse_output(output).unwrap();
        assert_eq!(resp.finish_reason, FinishReason::Length);
    }
}
