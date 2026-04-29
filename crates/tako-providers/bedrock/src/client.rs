//! Bedrock HTTP client + `LlmProvider` impl.

use std::sync::Arc;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::Client;
use futures::stream::BoxStream;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, LlmProvider, Principal, TakoError,
};

use crate::convert;
use crate::stream::into_chat_stream;

#[derive(Debug, Default, Clone)]
pub struct BedrockBuilder {
    model: Option<String>,
    region: Option<String>,
    endpoint_url: Option<String>,
    profile_name: Option<String>,
    capabilities: Option<Capabilities>,
}

impl BedrockBuilder {
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Override the Bedrock endpoint URL — useful for VPC-private
    /// endpoints and local mocks during testing.
    pub fn endpoint_url(mut self, url: impl Into<String>) -> Self {
        self.endpoint_url = Some(url.into());
        self
    }

    /// Pin a specific named AWS profile (defaults to whichever the
    /// credential chain selects).
    pub fn profile_name(mut self, name: impl Into<String>) -> Self {
        self.profile_name = Some(name.into());
        self
    }

    pub fn capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    /// Resolve credentials and build the provider. Loads the AWS config
    /// from the default credential chain (env, profile, IRSA, IMDS).
    pub async fn build(self) -> Result<BedrockProvider, TakoError> {
        let model = self
            .model
            .ok_or_else(|| TakoError::Invalid("BedrockBuilder: model is required".into()))?;

        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(r) = &self.region {
            loader = loader.region(aws_config::Region::new(r.clone()));
        }
        if let Some(p) = &self.profile_name {
            loader = loader.profile_name(p.clone());
        }
        if let Some(url) = &self.endpoint_url {
            loader = loader.endpoint_url(url.clone());
        }
        let shared = loader.load().await;
        let client = Client::new(&shared);

        let id = format!("bedrock:{model}");
        let capabilities = self.capabilities.unwrap_or(Capabilities {
            max_context_tokens: 200_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            supports_json_mode: false,
            usd_per_input_mtok: None,
            usd_per_output_mtok: None,
        });

        Ok(BedrockProvider {
            inner: Arc::new(Inner {
                id,
                model,
                client,
                capabilities,
            }),
        })
    }
}

#[derive(Debug)]
struct Inner {
    id: String,
    model: String,
    client: Client,
    capabilities: Capabilities,
}

#[derive(Clone, Debug)]
pub struct BedrockProvider {
    inner: Arc<Inner>,
}

impl BedrockProvider {
    pub fn builder() -> BedrockBuilder {
        BedrockBuilder::default()
    }
}

#[async_trait]
impl LlmProvider for BedrockProvider {
    fn id(&self) -> &str {
        &self.inner.id
    }

    fn capabilities(&self) -> &Capabilities {
        &self.inner.capabilities
    }

    async fn chat(
        &self,
        _principal: &Principal,
        mut req: ChatRequest,
    ) -> Result<ChatResponse, TakoError> {
        if req.model.is_empty() {
            req.model.clone_from(&self.inner.model);
        }
        let inputs = convert::to_converse_inputs(&req)?;

        let mut call = self
            .inner
            .client
            .converse()
            .model_id(&self.inner.model)
            .set_messages(Some(inputs.messages));
        if !inputs.system.is_empty() {
            call = call.set_system(Some(inputs.system));
        }
        if let Some(cfg) = inputs.inference_config {
            call = call.inference_config(cfg);
        }
        if let Some(tc) = inputs.tool_config {
            call = call.tool_config(tc);
        }

        let output = call
            .send()
            .await
            .map_err(|e| map_sdk_error(&self.inner.id, &self.inner.model, e))?;
        convert::from_converse_output(output)
    }

    async fn stream(
        &self,
        _principal: &Principal,
        mut req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        if req.model.is_empty() {
            req.model.clone_from(&self.inner.model);
        }
        let inputs = convert::to_converse_inputs(&req)?;

        let mut call = self
            .inner
            .client
            .converse_stream()
            .model_id(&self.inner.model)
            .set_messages(Some(inputs.messages));
        if !inputs.system.is_empty() {
            call = call.set_system(Some(inputs.system));
        }
        if let Some(cfg) = inputs.inference_config {
            call = call.inference_config(cfg);
        }
        if let Some(tc) = inputs.tool_config {
            call = call.tool_config(tc);
        }

        let output = call
            .send()
            .await
            .map_err(|e| map_sdk_error(&self.inner.id, &self.inner.model, e))?;
        Ok(into_chat_stream(output))
    }
}

fn map_sdk_error<E: std::fmt::Display + std::fmt::Debug>(
    provider_id: &str,
    model: &str,
    e: E,
) -> TakoError {
    TakoError::provider(provider_id, model, format!("{e}"))
}
