//! Bedrock HTTP client + `LlmProvider` impl.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::Client;
use futures::stream::BoxStream;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, LlmProvider, Principal, TakoError,
};

use crate::convert;
use crate::stream::into_chat_stream;
use crate::url_prefetch::{UrlPrefetchConfig, UrlPrefetchOpts};

#[derive(Debug, Default, Clone)]
pub struct BedrockBuilder {
    model: Option<String>,
    region: Option<String>,
    endpoint_url: Option<String>,
    profile_name: Option<String>,
    capabilities: Option<Capabilities>,
    url_prefetch: UrlPrefetchOpts,
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

    /// Phase 28.A — opt in to tako-side pre-fetch for
    /// `ContentPart::ImageUrl` content. Bedrock's `ImageSource`
    /// has no URL variant, so URL-source images require tako to
    /// fetch the bytes and pass them inline.
    ///
    /// Default behaviour is silent-drop (Phase 22.A semantics).
    /// SSRF mitigations baked in: `https://`-only by default
    /// (opt-in to `http://` via
    /// [`Self::with_url_prefetch_allow_http`]); 10s timeout
    /// (override via [`Self::with_url_prefetch_timeout`]); 10
    /// MiB response-size cap (override via
    /// [`Self::with_url_prefetch_max_bytes`]); MIME validated
    /// against the four `image/{jpeg,png,gif,webp}` types
    /// Bedrock accepts.
    ///
    /// Operators must enforce network egress at deployment level
    /// (VPC egress rules, Pod-level egress NetworkPolicies) for
    /// defence in depth — Phase 28 does not include CIDR-block
    /// or DNS-rebinding mitigation.
    pub fn with_url_prefetch(mut self) -> Self {
        self.url_prefetch.enabled = true;
        self
    }

    /// Phase 28.A — opt in to `http://` URLs alongside `https://`.
    /// Useful for internal artifact servers. Implies
    /// [`Self::with_url_prefetch`].
    pub fn with_url_prefetch_allow_http(mut self) -> Self {
        self.url_prefetch.enabled = true;
        self.url_prefetch.allow_http = true;
        self
    }

    /// Phase 28.A — override the default 10s connect+read timeout
    /// for URL pre-fetch. Implies [`Self::with_url_prefetch`].
    pub fn with_url_prefetch_timeout(mut self, timeout: Duration) -> Self {
        self.url_prefetch.enabled = true;
        self.url_prefetch.timeout = Some(timeout);
        self
    }

    /// Phase 28.A — override the default 10 MiB response-size
    /// cap. Implies [`Self::with_url_prefetch`].
    pub fn with_url_prefetch_max_bytes(mut self, max_bytes: usize) -> Self {
        self.url_prefetch.enabled = true;
        self.url_prefetch.max_bytes = Some(max_bytes);
        self
    }

    /// Phase 30 / 31 — add a hostname or wildcard pattern to
    /// the URL pre-fetch allowlist.
    ///
    /// Hosts matched by the allowlist bypass the private-IP
    /// blocklist (Phase 29.A) for that host only — but the
    /// scheme check, timeout, size cap, and MIME validation
    /// still apply. Useful for permitting an internal artifact
    /// registry on a private RFC 1918 address while keeping the
    /// rest of the blocklist active.
    ///
    /// Chainable; can be called multiple times to add more
    /// hosts. Two match modes:
    ///
    /// - **Exact string** (Phase 30) — `"registry.corp"` matches
    ///   `https://registry.corp/cat.png` but NOT
    ///   `https://other.corp/cat.png`. For IP-literal URLs,
    ///   match against the raw IP string (e.g. `"10.0.5.4"`).
    ///
    /// - **Wildcard suffix** (Phase 31) — `"*.internal.corp"`
    ///   matches any hostname ending with `.internal.corp`,
    ///   including multi-level subdomains
    ///   (`registry.internal.corp`,
    ///   `staging.images.internal.corp`). Does NOT match the
    ///   bare apex (`internal.corp`); add the apex as a
    ///   separate exact entry if needed. Wildcards must be the
    ///   leftmost label only (`"*.X"`); patterns like `"X.*.Y"`
    ///   are not supported.
    ///
    /// Does NOT auto-enable [`Self::with_url_prefetch`] — the
    /// master switch must already be on for this flag to have
    /// any effect.
    pub fn with_url_prefetch_allow_host(mut self, host: impl Into<String>) -> Self {
        self.url_prefetch.allow_hosts.push(host.into());
        self
    }

    /// Phase 32 — add a CIDR network to the URL pre-fetch
    /// allowlist. Both IPv4 (`"10.0.5.0/24"`) and IPv6
    /// (`"2001:db8::/32"`) CIDRs are accepted; single hosts as
    /// `/32` (IPv4) or `/128` (IPv6) work too.
    ///
    /// Bypass triggers when a resolved IP (or IP literal in the
    /// URL) falls inside any allowlisted CIDR — useful for
    /// permitting a whole private subnet without enumerating
    /// every host. The scheme / timeout / size / MIME checks
    /// still apply.
    ///
    /// CIDR parse failures surface from [`Self::build`] as
    /// `TakoError::Invalid` so operators notice early.
    /// Chainable; can be called multiple times. Does NOT
    /// auto-enable [`Self::with_url_prefetch`] — the master
    /// switch must already be on.
    pub fn with_url_prefetch_allow_cidr(mut self, cidr: impl Into<String>) -> Self {
        self.url_prefetch.allow_cidrs.push(cidr.into());
        self
    }

    /// Phase 29.A — opt out of the default-on private-IP blocklist
    /// for tako-side URL pre-fetch.
    ///
    /// By default, the URL pre-fetcher rejects URLs that resolve to
    /// loopback / RFC 1918 / link-local / multicast / IPv6 unique-
    /// local / IPv6 link-local addresses (and IPv4-mapped variants
    /// of those). The check runs at DNS-resolve time via a custom
    /// [`reqwest::dns::Resolve`] impl, validating EVERY returned
    /// address — which also closes the DNS-rebinding window.
    ///
    /// Operators with deployment-level egress filtering (VPC egress
    /// rules, Pod-level egress NetworkPolicies) can flip this off
    /// to allow internal artifact servers behind private addresses.
    /// Does NOT auto-enable [`Self::with_url_prefetch`] — the master
    /// switch must already be on for this flag to have any effect.
    pub fn with_url_prefetch_allow_private_ips(mut self) -> Self {
        self.url_prefetch.block_private_ips = false;
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

        // Phase 28.A — build the URL-prefetch reqwest client now
        // (eager) so PEM-style failures or invalid timeout settings
        // surface at builder time rather than at first request.
        let url_prefetch = self.url_prefetch.into_config()?;

        Ok(BedrockProvider {
            inner: Arc::new(Inner {
                id,
                model,
                client,
                capabilities,
                url_prefetch,
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
    /// Phase 28.A — opt-in URL pre-fetch config. `None` when the
    /// builder wasn't called with [`BedrockBuilder::with_url_prefetch`];
    /// in that case `ContentPart::ImageUrl` content is silently
    /// dropped at convert time (Phase 22.A semantics).
    url_prefetch: Option<UrlPrefetchConfig>,
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
        // Phase 28.A — when the operator opted in, pre-fetch any
        // `ContentPart::ImageUrl` content and rewrite to inline
        // `ContentPart::Image { mime, data_b64 }` before convert.
        if let Some(prefetch) = &self.inner.url_prefetch {
            prefetch.rewrite(&mut req).await?;
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
        // Phase 28.A — same pre-fetch pre-pass as `chat()`.
        if let Some(prefetch) = &self.inner.url_prefetch {
            prefetch.rewrite(&mut req).await?;
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
