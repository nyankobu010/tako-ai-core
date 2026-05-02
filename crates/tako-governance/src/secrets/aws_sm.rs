//! AWS Secrets Manager resolver.
//!
//! Uses the AWS SDK (`aws-sdk-secretsmanager`) so the standard AWS
//! credential chain (env, profile, IRSA, IMDS) Just Works. The dep tree
//! is shared with the Bedrock provider via `aws-config`, so this adds
//! only one new SDK crate to the workspace.

use std::time::Duration;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_secretsmanager::Client as SmClient;
use aws_sdk_secretsmanager::config::Region;
use tako_core::TakoError;
use tokio::sync::OnceCell;

use super::{SecretResolver, SecretString};

/// Resolver against AWS Secrets Manager.
///
/// Credential resolution is deferred until the first `resolve()` call so
/// constructing the resolver in tests doesn't require AWS creds.
#[derive(Clone)]
pub struct AwsSecretsManagerResolver {
    region: Option<String>,
    profile_name: Option<String>,
    endpoint_url: Option<String>,
    client: std::sync::Arc<OnceCell<SmClient>>,
}

impl std::fmt::Debug for AwsSecretsManagerResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsSecretsManagerResolver")
            .field("region", &self.region)
            .field("profile_name", &self.profile_name)
            .field("endpoint_url", &self.endpoint_url)
            .finish()
    }
}

impl AwsSecretsManagerResolver {
    /// Default resolver — picks region + credentials from the standard AWS
    /// chain.
    pub fn new() -> Self {
        Self {
            region: None,
            profile_name: None,
            endpoint_url: None,
            client: std::sync::Arc::new(OnceCell::new()),
        }
    }

    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile_name = Some(profile.into());
        self
    }

    /// Override the Secrets Manager endpoint URL. Useful for tests
    /// (LocalStack / a wiremock-fronted endpoint) or for VPC-private
    /// endpoints.
    pub fn with_endpoint_url(mut self, url: impl Into<String>) -> Self {
        self.endpoint_url = Some(url.into());
        self
    }

    async fn client(&self) -> Result<&SmClient, TakoError> {
        self.client
            .get_or_try_init(|| async {
                let mut loader = aws_config::defaults(BehaviorVersion::latest());
                if let Some(r) = &self.region {
                    loader = loader.region(Region::new(r.clone()));
                }
                if let Some(p) = &self.profile_name {
                    loader = loader.profile_name(p.clone());
                }
                if let Some(u) = &self.endpoint_url {
                    loader = loader.endpoint_url(u);
                }
                let cfg = tokio::time::timeout(Duration::from_secs(10), loader.load())
                    .await
                    .map_err(|_| TakoError::Transport("aws config load timed out".into()))?;
                Ok::<_, TakoError>(SmClient::new(&cfg))
            })
            .await
    }
}

impl Default for AwsSecretsManagerResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecretResolver for AwsSecretsManagerResolver {
    async fn resolve(&self, key: &str) -> Result<SecretString, TakoError> {
        // Optional version: `secret-arn-or-name#version-id`.
        let (secret_id, version_id) = match key.split_once('#') {
            Some((s, v)) => (s, Some(v)),
            None => (key, None),
        };
        let client = self.client().await?;
        let mut req = client.get_secret_value().secret_id(secret_id);
        if let Some(v) = version_id {
            req = req.version_id(v);
        }
        let out = req.send().await.map_err(|e| {
            TakoError::provider("aws-secrets-manager", secret_id.to_string(), format!("{e}"))
        })?;

        if let Some(s) = out.secret_string() {
            return Ok(SecretString::new(s));
        }
        if let Some(blob) = out.secret_binary() {
            // Surface binary secrets as base64 so callers see the same
            // shape as AWS's CLI/SDKs would return for a SecretBinary.
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(blob.as_ref());
            return Ok(SecretString::new(b64));
        }
        Err(TakoError::NotFound(format!(
            "aws secret `{secret_id}` had no string or binary value"
        )))
    }
}
