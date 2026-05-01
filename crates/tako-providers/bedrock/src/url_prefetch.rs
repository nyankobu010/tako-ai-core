//! Phase 28.A — opt-in tako-side URL-source image pre-fetch
//! for Bedrock.
//!
//! Bedrock's `ImageSource` accepts only `Bytes` (no URL variant),
//! so URL-source content (`ContentPart::ImageUrl`) requires
//! tako-side pre-fetch. This is the "vendor doesn't fetch URLs
//! themselves" sibling of Phases 22.B/C/D (Anthropic/OpenAI/
//! Mistral pass-through) and Phase 23.A (Vertex `fileData`).
//!
//! ## Security
//!
//! Tako-side fetch raises SSRF risk: an attacker who can inject
//! a `ContentPart::ImageUrl` into a request can ask tako to
//! fetch arbitrary URLs. Mitigations baked in:
//!
//! - **Opt-in.** [`BedrockBuilder::with_url_prefetch`] is
//!   required; default behaviour is silent-drop (Phase 22.A
//!   semantics).
//! - **`https://`-only by default.** `http://`, `gs://`,
//!   `file://`, etc. are rejected at the URL parse stage.
//!   Opt in to `http://` via
//!   [`BedrockBuilder::with_url_prefetch_allow_http`] for
//!   internal artifact servers.
//! - **Connect+read timeout.** Configurable; defaults to 10s.
//! - **Response size cap.** Configurable; defaults to 10 MiB.
//!   Enforced via `Content-Length` + post-fetch byte-length.
//! - **MIME validation.** `Content-Type` must be one of the
//!   four MIMEs Bedrock's `ImageFormat` enum accepts.
//!
//! Out of scope for Phase 28: CIDR blocklist for private /
//! link-local / loopback IPs, DNS-rebinding mitigation. Operators
//! must enforce network egress at deployment level (VPC egress
//! rules, Pod-level egress NetworkPolicies). Phase 29+ may add
//! per-request CIDR check + resolve-once-then-connect.

use std::time::Duration;

use tako_core::{ChatRequest, ContentPart, TakoError};

/// Phase 28.A — default `https`-only when not explicitly enabled.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Phase 28.A — default 10 MiB response-size cap.
const DEFAULT_MAX_BYTES: usize = 10 * 1024 * 1024;

/// Phase 28.A — pre-fetch configuration. Held by `BedrockProvider`
/// when [`BedrockBuilder::with_url_prefetch`] was called at build
/// time.
#[derive(Debug)]
pub(crate) struct UrlPrefetchConfig {
    pub(crate) allow_http: bool,
    pub(crate) max_bytes: usize,
    pub(crate) http: reqwest::Client,
}

impl UrlPrefetchConfig {
    pub(crate) fn new(
        allow_http: bool,
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<Self, TakoError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| {
                TakoError::Invalid(format!("bedrock: failed to build prefetch client: {e}"))
            })?;
        Ok(Self {
            allow_http,
            max_bytes,
            http,
        })
    }

    /// Walk `req.messages`, fetch each `ContentPart::ImageUrl` in
    /// place, rewrite to `ContentPart::Image { mime, data_b64 }`
    /// using base64-encoded bytes. Errors short-circuit the whole
    /// request — partial rewrites would leave the request in an
    /// inconsistent state.
    pub(crate) async fn rewrite(&self, req: &mut ChatRequest) -> Result<(), TakoError> {
        for message in &mut req.messages {
            for part in &mut message.content {
                if let ContentPart::ImageUrl { url, mime: _ } = part {
                    let (mime, bytes) = self.fetch_one(url).await?;
                    // Use `aws_smithy_types::base64::encode` —
                    // already a transitive dep via the AWS SDK; the
                    // matching `decode` is what `convert.rs` uses
                    // for the inverse operation.
                    let data_b64 = aws_smithy_types::base64::encode(&bytes);
                    *part = ContentPart::Image { mime, data_b64 };
                }
            }
        }
        Ok(())
    }

    async fn fetch_one(&self, url: &str) -> Result<(String, Vec<u8>), TakoError> {
        // Phase 28.A — scheme check: reject anything except
        // `https://` unless the operator explicitly opted in to
        // `http://` via `with_url_prefetch_allow_http`. Use
        // `reqwest::Url` (re-export of `url::Url`) so we don't
        // need a direct dep on the `url` crate.
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| TakoError::Invalid(format!("bedrock: prefetch URL parse failed: {e}")))?;
        let scheme = parsed.scheme();
        let scheme_ok = scheme == "https" || (self.allow_http && scheme == "http");
        if !scheme_ok {
            return Err(TakoError::Invalid(format!(
                "bedrock: prefetch URL scheme `{scheme}` rejected (only `https://` allowed; \
                 use `with_url_prefetch_allow_http` to allow `http://`)",
            )));
        }

        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| TakoError::Transport(format!("bedrock: prefetch GET {url}: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(TakoError::Invalid(format!(
                "bedrock: prefetch GET {url} returned {status}: {body}",
            )));
        }

        // Phase 28.A — Content-Length pre-check (cheap reject for
        // oversized servers).
        if let Some(content_length) = resp.content_length() {
            let len_usize = usize::try_from(content_length).unwrap_or(usize::MAX);
            if len_usize > self.max_bytes {
                return Err(TakoError::Invalid(format!(
                    "bedrock: prefetch response Content-Length {len_usize} exceeds max {}",
                    self.max_bytes,
                )));
            }
        }

        // MIME validation.
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if !is_supported_bedrock_mime(&content_type) {
            return Err(TakoError::Invalid(format!(
                "bedrock: prefetch Content-Type `{content_type}` not supported \
                 (need one of image/jpeg, image/png, image/gif, image/webp)",
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| TakoError::Transport(format!("bedrock: prefetch read body: {e}")))?;

        // Defence-in-depth: Content-Length may have been absent or
        // misreported; cap on actual bytes too.
        if bytes.len() > self.max_bytes {
            return Err(TakoError::Invalid(format!(
                "bedrock: prefetch response body {} bytes exceeds max {}",
                bytes.len(),
                self.max_bytes,
            )));
        }

        Ok((content_type, bytes.to_vec()))
    }
}

/// Phase 28.A — accept only the four MIME types Bedrock's
/// `ImageFormat` enum maps in `convert.rs`.
fn is_supported_bedrock_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Phase 28.A — builder-side knobs collected on
/// [`BedrockBuilder`]. Held opaquely until `build()`.
#[derive(Debug, Clone, Default)]
pub(crate) struct UrlPrefetchOpts {
    pub(crate) enabled: bool,
    pub(crate) allow_http: bool,
    pub(crate) timeout: Option<Duration>,
    pub(crate) max_bytes: Option<usize>,
}

impl UrlPrefetchOpts {
    pub(crate) fn into_config(self) -> Result<Option<UrlPrefetchConfig>, TakoError> {
        if !self.enabled {
            return Ok(None);
        }
        let cfg = UrlPrefetchConfig::new(
            self.allow_http,
            self.timeout.unwrap_or(DEFAULT_TIMEOUT),
            self.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
        )?;
        Ok(Some(cfg))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tako_core::{ChatRequest, Message, Role};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 1×1 transparent PNG fixture (smallest valid PNG).
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn req_with_image_url(url: &str) -> ChatRequest {
        ChatRequest::new(
            "test-model",
            vec![Message {
                role: Role::User,
                content: vec![ContentPart::ImageUrl {
                    url: url.to_string(),
                    mime: None,
                }],
            }],
        )
    }

    #[test]
    fn supported_mime_smoke() {
        for ok in ["image/jpeg", "image/png", "image/gif", "image/webp"] {
            assert!(is_supported_bedrock_mime(ok));
        }
        for bad in ["image/svg+xml", "text/plain", ""] {
            assert!(!is_supported_bedrock_mime(bad));
        }
    }

    #[tokio::test]
    async fn rewrite_fetches_image_and_emits_inline_base64() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cat.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(TINY_PNG),
            )
            .expect(1)
            .mount(&server)
            .await;

        let cfg = UrlPrefetchConfig::new(
            true, // allow_http for wiremock
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
        )
        .unwrap();
        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        cfg.rewrite(&mut req).await.unwrap();

        // Verify the rewrite landed.
        let part = &req.messages[0].content[0];
        match part {
            ContentPart::Image { mime, data_b64 } => {
                assert_eq!(mime, "image/png");
                let decoded = aws_smithy_types::base64::decode(data_b64).unwrap();
                assert_eq!(decoded, TINY_PNG);
            }
            other => panic!("expected ContentPart::Image, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rewrite_rejects_http_url_by_default() {
        let cfg = UrlPrefetchConfig::new(
            false, // allow_http = false (default)
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
        )
        .unwrap();
        let mut req = req_with_image_url("http://example.com/cat.png");
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("scheme `http` rejected"), "got: {msg}");
    }

    #[tokio::test]
    async fn rewrite_rejects_unsupported_mime() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/icon.svg"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(b"<svg/>".as_slice(), "image/svg+xml"),
            )
            .mount(&server)
            .await;

        let cfg = UrlPrefetchConfig::new(true, DEFAULT_TIMEOUT, DEFAULT_MAX_BYTES).unwrap();
        let mut req = req_with_image_url(&format!("{}/icon.svg", server.uri()));
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("image/svg+xml"), "got: {msg}");
        assert!(msg.contains("not supported"), "got: {msg}");
    }

    #[tokio::test]
    async fn rewrite_rejects_oversized_response_via_content_length() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/big.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    // wiremock sets Content-Length automatically based on body length.
                    .set_body_bytes(vec![0u8; 1024]),
            )
            .mount(&server)
            .await;

        // Cap at 100 bytes — server returns 1024.
        let cfg = UrlPrefetchConfig::new(true, DEFAULT_TIMEOUT, 100).unwrap();
        let mut req = req_with_image_url(&format!("{}/big.png", server.uri()));
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("exceeds max"), "got: {msg}");
    }

    #[tokio::test]
    async fn rewrite_propagates_5xx_as_invalid() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/oops"))
            .respond_with(ResponseTemplate::new(503).set_body_string("oops"))
            .mount(&server)
            .await;

        let cfg = UrlPrefetchConfig::new(true, DEFAULT_TIMEOUT, DEFAULT_MAX_BYTES).unwrap();
        let mut req = req_with_image_url(&format!("{}/oops", server.uri()));
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("503"), "got: {msg}");
    }

    #[tokio::test]
    async fn opts_into_config_disabled_returns_none() {
        let opts = UrlPrefetchOpts::default();
        let cfg = opts.into_config().unwrap();
        assert!(cfg.is_none());
    }

    #[tokio::test]
    async fn opts_into_config_enabled_returns_some() {
        let opts = UrlPrefetchOpts {
            enabled: true,
            ..UrlPrefetchOpts::default()
        };
        let cfg = opts.into_config().unwrap();
        assert!(cfg.is_some());
    }
}
