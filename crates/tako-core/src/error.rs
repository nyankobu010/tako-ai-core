//! `TakoError` — the unified error type for the framework. Every fallible
//! API in `tako-core` and downstream crates returns `Result<_, TakoError>`.

use std::time::Duration;

/// Structured details preserved alongside vendor errors so we never lose
/// the original status code or response body.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ProviderErrorDetails {
    pub provider_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_body: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum TakoError {
    #[error("provider error: {message}")]
    Provider {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
        details: Box<ProviderErrorDetails>,
    },

    #[error("transport error: {0}")]
    Transport(String),

    #[error("policy denied: {0}")]
    PolicyDenied(String),

    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),

    #[error("rate limited: retry in {0:?}")]
    RateLimited(Duration),

    #[error("circuit open")]
    CircuitOpen,

    #[error("tool error: {0}")]
    Tool(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("cancelled")]
    Cancelled,

    #[error("timeout after {0:?}")]
    Timeout(Duration),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

impl TakoError {
    /// Construct a `Provider` error preserving the vendor context.
    pub fn provider(
        provider_id: impl Into<String>,
        model: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Provider {
            message: message.into(),
            source: None,
            details: Box::new(ProviderErrorDetails {
                provider_id: provider_id.into(),
                model: model.into(),
                status_code: None,
                vendor_error_code: None,
                raw_body: None,
            }),
        }
    }

    /// Attach an HTTP status code to a `Provider` error.
    pub fn with_status(mut self, status: u16) -> Self {
        if let Self::Provider { details, .. } = &mut self {
            details.status_code = Some(status);
        }
        self
    }

    /// Attach a raw response body to a `Provider` error.
    pub fn with_raw_body(mut self, body: impl Into<String>) -> Self {
        if let Self::Provider { details, .. } = &mut self {
            details.raw_body = Some(body.into());
        }
        self
    }

    /// Whether this error is worth retrying against the same provider.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::RateLimited(_) | Self::Timeout(_) | Self::Transport(_) | Self::CircuitOpen
        ) || matches!(
            self,
            Self::Provider { details, .. } if matches!(details.status_code, Some(s) if (500..600).contains(&s))
        )
    }
}

/// Convert from `reqwest::Error` lives in provider crates (we don't depend
/// on reqwest here). This stub conversion exists so callers in this crate
/// can still return `TakoError` from generic IO paths.
impl From<&str> for TakoError {
    fn from(s: &str) -> Self {
        Self::Invalid(s.into())
    }
}
