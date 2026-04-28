//! `SingleAgent` orchestrator: one provider + a tool registry + a
//! max-step loop. Halts on a model response with no tool calls (final
//! answer) or when `max_steps` is reached.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tako_core::{
    ChatRequest, ContentPart, FinishReason, LlmProvider, Message, Principal, Role, TakoError, Usage,
};
use tako_mcp::ToolRegistry;
use tracing::{Instrument, info_span};

use crate::types::{OrchEvent, OrchInput, OrchOutput};
use crate::{Orchestrator, OrchestratorKind};

const DEFAULT_MAX_STEPS: u32 = 8;

/// Single-agent orchestrator.
pub struct SingleAgent {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    max_steps: u32,
    /// Optional pre-set defaults for ChatRequest fields the orchestrator
    /// constructs (temperature, max_tokens). Tools and stream are managed
    /// by the orchestrator itself.
    defaults: ChatDefaults,
}

#[derive(Clone, Debug, Default)]
pub struct ChatDefaults {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl std::fmt::Debug for SingleAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleAgent")
            .field("provider", &self.provider.id())
            .field("max_steps", &self.max_steps)
            .finish()
    }
}

impl SingleAgent {
    pub fn builder() -> SingleAgentBuilder {
        SingleAgentBuilder::default()
    }
}

#[derive(Default)]
pub struct SingleAgentBuilder {
    provider: Option<Arc<dyn LlmProvider>>,
    tools: Option<Arc<ToolRegistry>>,
    max_steps: Option<u32>,
    defaults: ChatDefaults,
}

impl std::fmt::Debug for SingleAgentBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleAgentBuilder")
            .field("provider", &self.provider.as_ref().map(|p| p.id()))
            .field("max_steps", &self.max_steps)
            .field("defaults", &self.defaults)
            .finish_non_exhaustive()
    }
}

impl SingleAgentBuilder {
    pub fn provider(mut self, provider: Arc<dyn LlmProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn max_steps(mut self, n: u32) -> Self {
        self.max_steps = Some(n.max(1));
        self
    }

    pub fn temperature(mut self, t: f32) -> Self {
        self.defaults.temperature = Some(t);
        self
    }

    pub fn max_tokens(mut self, n: u32) -> Self {
        self.defaults.max_tokens = Some(n);
        self
    }

    pub fn build(self) -> Result<SingleAgent, TakoError> {
        Ok(SingleAgent {
            provider: self.provider.ok_or_else(|| {
                TakoError::Invalid("SingleAgentBuilder: provider is required".into())
            })?,
            tools: self.tools.unwrap_or_else(|| Arc::new(ToolRegistry::new())),
            max_steps: self.max_steps.unwrap_or(DEFAULT_MAX_STEPS),
            defaults: self.defaults,
        })
    }
}

impl SingleAgent {
    fn build_request(
        &self,
        model: &str,
        messages: Vec<Message>,
        tool_schemas: Vec<tako_core::ToolSchema>,
    ) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages,
            tools: tool_schemas,
            temperature: self.defaults.temperature,
            max_tokens: self.defaults.max_tokens,
            stop: Vec::new(),
            stream: false,
            metadata: Default::default(),
        }
    }
}

#[async_trait]
impl Orchestrator for SingleAgent {
    fn kind(&self) -> OrchestratorKind {
        OrchestratorKind::SingleAgent
    }

    async fn run(&self, principal: &Principal, input: OrchInput) -> Result<OrchOutput, TakoError> {
        let span = info_span!(
            "tako.orchestrator.run",
            "tako.orchestrator.kind" = "single",
            "tako.principal.tenant_id" = %principal.tenant_id,
            "tako.principal.user_id" = %principal.user_id,
        );
        async move {
            let mut messages: Vec<Message> = Vec::new();
            if let Some(sys) = input.system.clone() {
                messages.push(Message::system(sys));
            }
            messages.extend(input.messages);

            let model = self
                .provider
                .id()
                .split(':')
                .nth(1)
                .unwrap_or("")
                .to_string();
            let mut total_usage = Usage::default();
            let mut steps = 0_u32;
            let mut final_message: Option<Message> = None;

            for step in 0..self.max_steps {
                let tool_schemas = self.tools.schemas().await;
                let req = self.build_request(&model, messages.clone(), tool_schemas);
                let resp = {
                    let span = info_span!(
                        "tako.provider.chat",
                        "tako.provider.id" = %self.provider.id(),
                        "tako.provider.model" = %model,
                        "tako.orchestrator.step" = step,
                    );
                    self.provider.chat(principal, req).instrument(span).await?
                };
                steps += 1;
                total_usage.input_tokens = total_usage
                    .input_tokens
                    .saturating_add(resp.usage.input_tokens);
                total_usage.output_tokens = total_usage
                    .output_tokens
                    .saturating_add(resp.usage.output_tokens);

                let assistant = resp.message.clone();
                messages.push(assistant.clone());

                let tool_calls: Vec<(String, String, serde_json::Value)> = assistant
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ContentPart::ToolCall { id, name, args } => {
                            Some((id.clone(), name.clone(), args.clone()))
                        }
                        _ => None,
                    })
                    .collect();

                if tool_calls.is_empty()
                    || matches!(
                        resp.finish_reason,
                        FinishReason::Stop | FinishReason::Length
                    ) && tool_calls.is_empty()
                {
                    final_message = Some(assistant);
                    break;
                }

                // Execute each tool call in order; append results as a single
                // user-role message containing all ToolResult parts.
                let mut tool_results: Vec<ContentPart> = Vec::with_capacity(tool_calls.len());
                for (id, name, args) in tool_calls {
                    let result = match self.tools.invoke(principal, &name, args).await {
                        Ok(v) => ContentPart::ToolResult {
                            id,
                            result: v,
                            is_error: false,
                        },
                        Err(e) => ContentPart::ToolResult {
                            id,
                            result: serde_json::json!({ "error": e.to_string() }),
                            is_error: true,
                        },
                    };
                    tool_results.push(result);
                }
                messages.push(Message {
                    role: Role::User,
                    content: tool_results,
                });

                if step + 1 == self.max_steps {
                    final_message = Some(assistant);
                }
            }

            let final_message = final_message.unwrap_or_else(|| Message::assistant(""));
            let text = final_message
                .content
                .iter()
                .filter_map(ContentPart::as_text)
                .collect::<Vec<_>>()
                .join("");

            Ok(OrchOutput {
                text,
                message: final_message,
                usage: total_usage,
                steps,
            })
        }
        .instrument(span)
        .await
    }

    async fn stream(
        &self,
        _principal: &Principal,
        _input: OrchInput,
    ) -> BoxStream<'static, Result<OrchEvent, TakoError>> {
        // Streaming events are Phase 2; for now `run` is sufficient and
        // python/tako wraps it.
        Box::pin(futures::stream::once(async {
            Err(TakoError::Invalid(
                "SingleAgent streaming is Phase 2".into(),
            ))
        }))
    }
}
