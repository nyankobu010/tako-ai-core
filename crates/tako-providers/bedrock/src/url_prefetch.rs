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
//! - **Phase 29.A — Private-IP blocklist + DNS-rebinding
//!   mitigation.** A custom [`reqwest::dns::Resolve`] impl runs
//!   at hostname-resolve time, validates EVERY returned IP
//!   against [`is_blocked_ip`] (loopback / RFC 1918 / link-local
//!   / multicast / IPv6 unique-local + link-local + IPv4-mapped
//!   variants), and rejects the resolution if any address is
//!   blocked. Default-on; opt out via
//!   [`BedrockBuilder::with_url_prefetch_allow_private_ips`] for
//!   deployments where the operator has already filtered network
//!   egress.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
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
    /// Phase 29.A — controls both: (a) the DNS resolver
    /// installed on `http` (for hostname URLs); (b) the
    /// IP-literal check inline in `fetch_one` (for URLs whose
    /// host is already an IP, where reqwest skips the resolver).
    pub(crate) block_private_ips: bool,
    /// Phase 30 — per-host allowlist. Hostnames in this set
    /// bypass the private-IP blocklist (but NOT the scheme /
    /// timeout / size / MIME checks). Empty by default.
    pub(crate) allow_hosts: Arc<HashSet<String>>,
    pub(crate) http: reqwest::Client,
}

impl UrlPrefetchConfig {
    pub(crate) fn new(
        allow_http: bool,
        timeout: Duration,
        max_bytes: usize,
        block_private_ips: bool,
        allow_hosts: Arc<HashSet<String>>,
    ) -> Result<Self, TakoError> {
        let mut builder = reqwest::Client::builder().timeout(timeout);
        if block_private_ips {
            builder = builder.dns_resolver(Arc::new(BlocklistResolver {
                allow_hosts: allow_hosts.clone(),
            }));
        }
        let http = builder.build().map_err(|e| {
            TakoError::Invalid(format!("bedrock: failed to build prefetch client: {e}"))
        })?;
        Ok(Self {
            allow_http,
            max_bytes,
            block_private_ips,
            allow_hosts,
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

        // Phase 29.A — inline IP-literal check. reqwest's DNS
        // resolver is only consulted for hostname URLs; when the
        // host is already an IP literal (e.g. `http://127.0.0.1/`)
        // reqwest connects directly without calling the resolver.
        // The blocklist must be enforced here too. Parse host_str
        // as IpAddr (stripping IPv6 brackets); on parse failure
        // it's a domain name and the resolver path takes over.
        //
        // Phase 30 — allowlisted host strings bypass the
        // blocklist. Match against the raw host_str (not the
        // parsed IpAddr) so `with_url_prefetch_allow_host("10.0.5.4")`
        // matches a URL whose host is exactly `10.0.5.4`.
        if self.block_private_ips {
            if let Some(host_str) = parsed.host_str() {
                let trimmed = host_str.trim_start_matches('[').trim_end_matches(']');
                if let Ok(ip) = trimmed.parse::<IpAddr>() {
                    if !self.allow_hosts.contains(host_str) && is_blocked_ip(&ip) {
                        return Err(TakoError::Invalid(format!(
                            "bedrock: prefetch URL `{url}` resolves to blocked IP `{ip}` \
                             (use `with_url_prefetch_allow_private_ips` to opt out, \
                             or `with_url_prefetch_allow_host` to permit this host)",
                        )));
                    }
                }
            }
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

/// Phase 29.A — return `true` for IPs that should be blocked by
/// the SSRF guard (loopback / private / link-local / multicast /
/// reserved / IPv6 equivalents). Pure stdlib; no new deps.
///
/// Used by [`BlocklistResolver`] at DNS-resolve time so blocked
/// IPs never reach the connector. Operators can opt out for
/// deployments where the network layer already filters egress
/// (`with_url_prefetch_allow_private_ips`).
pub(crate) fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                // Multicast (224/4) + reserved (240/4 except broadcast).
                // 224..=239 = multicast; 240..=255 = reserved/future-use.
                // Both are unrouted on the public Internet and should
                // not be reachable destinations for a URL fetch.
                || v4.octets()[0] >= 224
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // unique-local (fc00::/7): leading 7 bits = 1111110.
                || (v6.segments()[0] & 0xfe00 == 0xfc00)
                // unicast-link-local (fe80::/10): leading 10 bits = 1111111010.
                || (v6.segments()[0] & 0xffc0 == 0xfe80)
                // IPv4-mapped (::ffff:x.x.x.x): recurse on the embedded IPv4.
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_blocked_ip(&IpAddr::V4(v4)))
        }
    }
}

/// Phase 29.A — `reqwest::dns::Resolve` impl that wraps
/// `tokio::net::lookup_host` and rejects any resolution where
/// ANY returned `SocketAddr` fails [`is_blocked_ip`].
///
/// Validating every returned address (not just the first) closes
/// the DNS-rebinding window: a malicious resolver returning two
/// A records (one public, one private) can't slip the private IP
/// through alongside a public one, and there's no second
/// resolution between validation and connection.
///
/// Phase 30 — `allow_hosts` is the per-host bypass. When the
/// requested hostname is in this set, the blocklist is skipped
/// for that hostname only.
#[derive(Debug)]
struct BlocklistResolver {
    allow_hosts: Arc<HashSet<String>>,
}

impl Resolve for BlocklistResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let allow_hosts = self.allow_hosts.clone();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            // Phase 30 — bypass the blocklist for allowlisted hosts.
            if !allow_hosts.contains(&host) {
                for addr in &addrs {
                    if is_blocked_ip(&addr.ip()) {
                        return Err(Box::new(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!(
                                "prefetch URL `{host}` resolves to blocked IP `{}` \
                                 (use with_url_prefetch_allow_private_ips to opt out, \
                                 or with_url_prefetch_allow_host to permit this host)",
                                addr.ip()
                            ),
                        ))
                            as Box<dyn std::error::Error + Send + Sync>);
                    }
                }
            }
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

/// Phase 28.A — builder-side knobs collected on
/// [`BedrockBuilder`]. Held opaquely until `build()`.
#[derive(Debug, Clone)]
pub(crate) struct UrlPrefetchOpts {
    pub(crate) enabled: bool,
    pub(crate) allow_http: bool,
    pub(crate) timeout: Option<Duration>,
    pub(crate) max_bytes: Option<usize>,
    /// Phase 29.A — default `true` (block private / loopback /
    /// link-local IPs). Operators flip to `false` via
    /// [`BedrockBuilder::with_url_prefetch_allow_private_ips`] when
    /// deployment-level egress filtering is already enforced.
    pub(crate) block_private_ips: bool,
    /// Phase 30 — host strings that bypass the private-IP
    /// blocklist. Default empty. Populated via
    /// [`BedrockBuilder::with_url_prefetch_allow_host`].
    pub(crate) allow_hosts: Vec<String>,
}

impl Default for UrlPrefetchOpts {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_http: false,
            timeout: None,
            max_bytes: None,
            // Phase 29.A — default-deny stance for SSRF.
            block_private_ips: true,
            // Phase 30 — empty allowlist by default.
            allow_hosts: Vec::new(),
        }
    }
}

impl UrlPrefetchOpts {
    pub(crate) fn into_config(self) -> Result<Option<UrlPrefetchConfig>, TakoError> {
        if !self.enabled {
            return Ok(None);
        }
        let allow_hosts: Arc<HashSet<String>> = Arc::new(self.allow_hosts.into_iter().collect());
        let cfg = UrlPrefetchConfig::new(
            self.allow_http,
            self.timeout.unwrap_or(DEFAULT_TIMEOUT),
            self.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
            self.block_private_ips,
            allow_hosts,
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
            // Phase 29.A — wiremock binds to 127.0.0.1, which the
            // default blocklist would reject. Disable for this test;
            // dedicated DNS-blocklist tests live below.
            false,
            // Phase 30 — empty allowlist for this test.
            Arc::new(HashSet::new()),
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
            false, // block_private_ips off — scheme check fires first
            Arc::new(HashSet::new()),
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

        let cfg = UrlPrefetchConfig::new(
            true,
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            false,
            Arc::new(HashSet::new()),
        )
        .unwrap();
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
        let cfg =
            UrlPrefetchConfig::new(true, DEFAULT_TIMEOUT, 100, false, Arc::new(HashSet::new()))
                .unwrap();
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

        let cfg = UrlPrefetchConfig::new(
            true,
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            false,
            Arc::new(HashSet::new()),
        )
        .unwrap();
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

    // ----- Phase 29.A — `is_blocked_ip` + DNS-resolver tests. -----

    use std::net::Ipv4Addr;
    use std::net::Ipv6Addr;

    #[test]
    fn is_blocked_ip_blocks_loopback_v4() {
        for ip in ["127.0.0.1", "127.255.255.254"] {
            let addr = IpAddr::V4(ip.parse::<Ipv4Addr>().unwrap());
            assert!(is_blocked_ip(&addr), "expected {ip} blocked");
        }
    }

    #[test]
    fn is_blocked_ip_blocks_private_v4() {
        for ip in [
            "10.0.0.1",
            "10.255.255.255",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.0.1",
            "192.168.255.255",
        ] {
            let addr = IpAddr::V4(ip.parse::<Ipv4Addr>().unwrap());
            assert!(is_blocked_ip(&addr), "expected {ip} blocked");
        }
    }

    #[test]
    fn is_blocked_ip_blocks_link_local_v4() {
        // 169.254.169.254 is the AWS / GCP / Azure cloud-metadata
        // canary — the canonical SSRF target the blocklist exists
        // to protect.
        for ip in ["169.254.0.1", "169.254.169.254"] {
            let addr = IpAddr::V4(ip.parse::<Ipv4Addr>().unwrap());
            assert!(is_blocked_ip(&addr), "expected {ip} blocked");
        }
    }

    #[test]
    fn is_blocked_ip_blocks_unspecified_and_broadcast_v4() {
        for ip in ["0.0.0.0", "255.255.255.255"] {
            let addr = IpAddr::V4(ip.parse::<Ipv4Addr>().unwrap());
            assert!(is_blocked_ip(&addr), "expected {ip} blocked");
        }
    }

    #[test]
    fn is_blocked_ip_blocks_multicast_and_reserved_v4() {
        for ip in ["224.0.0.1", "239.255.255.255", "240.0.0.1"] {
            let addr = IpAddr::V4(ip.parse::<Ipv4Addr>().unwrap());
            assert!(is_blocked_ip(&addr), "expected {ip} blocked");
        }
    }

    #[test]
    fn is_blocked_ip_blocks_loopback_v6() {
        let addr = IpAddr::V6("::1".parse::<Ipv6Addr>().unwrap());
        assert!(is_blocked_ip(&addr));
    }

    #[test]
    fn is_blocked_ip_blocks_unique_local_v6() {
        for ip in ["fc00::1", "fd00::ffff", "fdff::1"] {
            let addr = IpAddr::V6(ip.parse::<Ipv6Addr>().unwrap());
            assert!(is_blocked_ip(&addr), "expected {ip} blocked");
        }
    }

    #[test]
    fn is_blocked_ip_blocks_link_local_v6() {
        for ip in ["fe80::1", "febf::ffff"] {
            let addr = IpAddr::V6(ip.parse::<Ipv6Addr>().unwrap());
            assert!(is_blocked_ip(&addr), "expected {ip} blocked");
        }
    }

    #[test]
    fn is_blocked_ip_blocks_v4_mapped_loopback() {
        // ::ffff:127.0.0.1 — IPv4-mapped IPv6, loopback variant.
        let addr = IpAddr::V6("::ffff:127.0.0.1".parse::<Ipv6Addr>().unwrap());
        assert!(is_blocked_ip(&addr));
    }

    #[test]
    fn is_blocked_ip_blocks_v4_mapped_private() {
        let addr = IpAddr::V6("::ffff:10.0.0.1".parse::<Ipv6Addr>().unwrap());
        assert!(is_blocked_ip(&addr));
    }

    #[test]
    fn is_blocked_ip_allows_public_v4() {
        for ip in ["8.8.8.8", "1.1.1.1", "151.101.65.140"] {
            let addr = IpAddr::V4(ip.parse::<Ipv4Addr>().unwrap());
            assert!(!is_blocked_ip(&addr), "expected {ip} ALLOWED");
        }
    }

    #[test]
    fn is_blocked_ip_allows_public_v6() {
        // 2001:db8::/32 is the documentation prefix — technically
        // reserved but not currently in the blocklist surface.
        // Public Internet IPv6 (e.g. Cloudflare 2606:4700::) is
        // also allowed.
        for ip in ["2001:db8::1", "2606:4700::1"] {
            let addr = IpAddr::V6(ip.parse::<Ipv6Addr>().unwrap());
            assert!(!is_blocked_ip(&addr), "expected {ip} ALLOWED");
        }
    }

    #[test]
    fn opts_default_blocks_private_ips() {
        let opts = UrlPrefetchOpts::default();
        assert!(opts.block_private_ips, "default should block private IPs");
    }

    #[tokio::test]
    async fn opts_into_config_can_allow_private_ips() {
        // When an operator opts out via with_url_prefetch_allow_private_ips,
        // the resolver should NOT be installed — verifying via the public
        // path that the build at least succeeds.
        let opts = UrlPrefetchOpts {
            enabled: true,
            block_private_ips: false,
            ..UrlPrefetchOpts::default()
        };
        let cfg = opts.into_config().unwrap();
        assert!(cfg.is_some());
    }

    /// Phase 29.A end-to-end: with the default-on blocklist, a
    /// pre-fetch request whose URL points at `127.0.0.1` (the
    /// wiremock loopback bind) must fail at DNS resolve, before
    /// any HTTP request is issued.
    #[tokio::test]
    async fn rewrite_rejects_resolved_loopback_ip_when_blocking() {
        let server = MockServer::start().await;
        // Mount a mock that would respond if reached. The
        // `expect(0)` invariant is asserted on drop — so we'll
        // also verify the server received zero requests.
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(TINY_PNG),
            )
            .expect(0)
            .mount(&server)
            .await;

        let cfg = UrlPrefetchConfig::new(
            true, // allow_http for wiremock
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            true, // block_private_ips ON — Phase 29.A default
            Arc::new(HashSet::new()),
        )
        .unwrap();

        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        // The rejection comes back as a transport error
        // (reqwest wraps the resolver error). Assert on the
        // distinctive substring from BlocklistResolver.
        assert!(
            msg.contains("blocked IP") || msg.contains("PermissionDenied"),
            "expected resolver rejection in error, got: {msg}",
        );
    }

    // ----- Phase 30 — per-host allowlist tests. -----

    #[test]
    fn opts_default_allow_hosts_is_empty() {
        let opts = UrlPrefetchOpts::default();
        assert!(
            opts.allow_hosts.is_empty(),
            "default allowlist should be empty"
        );
    }

    #[tokio::test]
    async fn opts_into_config_round_trips_allow_hosts() {
        let opts = UrlPrefetchOpts {
            enabled: true,
            allow_hosts: vec!["a.example".into(), "b.example".into()],
            ..UrlPrefetchOpts::default()
        };
        let cfg = opts.into_config().unwrap().expect("enabled => Some");
        assert_eq!(cfg.allow_hosts.len(), 2);
        assert!(cfg.allow_hosts.contains("a.example"));
        assert!(cfg.allow_hosts.contains("b.example"));
    }

    #[tokio::test]
    async fn opts_into_config_dedupes_allow_hosts() {
        let opts = UrlPrefetchOpts {
            enabled: true,
            allow_hosts: vec!["dup.example".into(), "dup.example".into()],
            ..UrlPrefetchOpts::default()
        };
        let cfg = opts.into_config().unwrap().expect("enabled => Some");
        // Vec<String> -> HashSet<String> dedupes by definition.
        assert_eq!(cfg.allow_hosts.len(), 1);
        assert!(cfg.allow_hosts.contains("dup.example"));
    }

    /// Phase 30 — IP-literal URL `127.0.0.1` is blocked by the
    /// default Phase 29 blocklist UNLESS the operator allowlists
    /// the literal host string.
    #[tokio::test]
    async fn rewrite_allowlists_ip_literal_host() {
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

        // Block-private-IPs ON, but `127.0.0.1` is in the allowlist.
        let mut allow_hosts = HashSet::new();
        allow_hosts.insert("127.0.0.1".to_string());

        let cfg = UrlPrefetchConfig::new(
            true, // allow_http for wiremock
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            true, // block_private_ips ON
            Arc::new(allow_hosts),
        )
        .unwrap();

        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        cfg.rewrite(&mut req).await.unwrap();

        // Verify the rewrite landed.
        match &req.messages[0].content[0] {
            ContentPart::Image { mime, data_b64 } => {
                assert_eq!(mime, "image/png");
                assert_eq!(
                    aws_smithy_types::base64::decode(data_b64).unwrap(),
                    TINY_PNG
                );
            }
            other => panic!("expected ContentPart::Image, got {other:?}"),
        }
    }

    /// Phase 30 — the allowlist matches the URL's host EXACTLY.
    /// A URL targeting `127.0.0.1` is NOT bypassed when only a
    /// different host (`some-other-host`) is in the allowlist.
    #[tokio::test]
    async fn rewrite_does_not_allowlist_other_hosts() {
        let server = MockServer::start().await;
        // Mount a mock that would respond if reached. The
        // `expect(0)` invariant asserts the request never arrived.
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(TINY_PNG),
            )
            .expect(0)
            .mount(&server)
            .await;

        // Block-private-IPs ON, allowlist contains a DIFFERENT host.
        let mut allow_hosts = HashSet::new();
        allow_hosts.insert("some-other-host".to_string());

        let cfg = UrlPrefetchConfig::new(
            true,
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            true,
            Arc::new(allow_hosts),
        )
        .unwrap();

        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("blocked IP") || msg.contains("PermissionDenied"),
            "expected blocklist rejection, got: {msg}",
        );
    }

    /// Phase 29.A regression pin: with the operator-opt-out flag,
    /// the same loopback URL succeeds. This confirms the kill switch
    /// works (no permanent block).
    #[tokio::test]
    async fn rewrite_allows_resolved_loopback_when_allow_private_ips_set() {
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
            true, // allow_http
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            false, // block_private_ips OFF — operator opt-out
            Arc::new(HashSet::new()),
        )
        .unwrap();

        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        cfg.rewrite(&mut req).await.unwrap();

        // Verify the rewrite landed (mirrors the Phase 28 happy-path).
        match &req.messages[0].content[0] {
            ContentPart::Image { mime, data_b64 } => {
                assert_eq!(mime, "image/png");
                assert_eq!(
                    aws_smithy_types::base64::decode(data_b64).unwrap(),
                    TINY_PNG
                );
            }
            other => panic!("expected ContentPart::Image, got {other:?}"),
        }
    }
}
