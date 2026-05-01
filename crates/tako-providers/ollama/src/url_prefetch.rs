//! Phase 28.B — opt-in tako-side URL-source image pre-fetch
//! for Ollama.
//!
//! Ollama's `/api/chat` endpoint accepts images only as bare
//! base64 in the `images: Vec<String>` sibling field on each
//! message (Phase 20.C). There's no URL variant, so URL-source
//! content (`ContentPart::ImageUrl`) requires tako-side pre-fetch.
//! This is the Ollama-side mirror of Phase 28.A on Bedrock —
//! same SSRF mitigations, same builder cadence.
//!
//! ## Security
//!
//! See [`crates/tako-providers/bedrock/src/url_prefetch.rs`] for
//! the full design rationale. tl;dr: opt-in via
//! [`OllamaBuilder::with_url_prefetch`]; `https://`-only by
//! default; 10s timeout; 10 MiB cap; MIME validated.
//!
//! Phase 29.B — private-IP blocklist + DNS-rebinding mitigation
//! mirror Phase 29.A on Bedrock. A custom
//! [`reqwest::dns::Resolve`] impl validates EVERY returned IP at
//! resolve time; an inline IP-literal check covers URLs whose
//! host is already an IP (where reqwest skips the resolver).
//! Default-on; opt out via
//! [`OllamaBuilder::with_url_prefetch_allow_private_ips`] for
//! deployments where the operator has already filtered network
//! egress.
//!
//! Ollama's MIME acceptance is more permissive than the four
//! formats Bedrock allows (Ollama passes raw bytes to the
//! underlying model, e.g. LLaVA, which decodes them itself).
//! Phase 28.B still validates the four common image MIMEs —
//! tako has no way to know what arbitrary MIME a given local
//! model can decode, so we conservatively match what the other
//! provider adapters accept.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tako_core::{ChatRequest, ContentPart, TakoError};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_BYTES: usize = 10 * 1024 * 1024;

/// Phase 28.B — pre-fetch configuration. Held by `OllamaProvider`
/// when [`OllamaBuilder::with_url_prefetch`] was called at build
/// time.
#[derive(Debug)]
pub(crate) struct UrlPrefetchConfig {
    pub(crate) allow_http: bool,
    pub(crate) max_bytes: usize,
    /// Phase 29.B — controls both: (a) the DNS resolver
    /// installed on `http` (for hostname URLs); (b) the
    /// IP-literal check inline in `fetch_one` (for URLs whose
    /// host is already an IP, where reqwest skips the resolver).
    pub(crate) block_private_ips: bool,
    /// Phase 30 / 31 — per-host allowlist. Hostnames matched by
    /// this set bypass the private-IP blocklist (but NOT the
    /// scheme / timeout / size / MIME checks). Phase 30 ships
    /// exact-string match; Phase 31 adds wildcard suffix
    /// patterns (`*.X` matches any host ending in `.X`,
    /// including multi-level subdomains).
    pub(crate) allow_hosts: Arc<AllowList>,
    pub(crate) http: reqwest::Client,
}

impl UrlPrefetchConfig {
    pub(crate) fn new(
        allow_http: bool,
        timeout: Duration,
        max_bytes: usize,
        block_private_ips: bool,
        allow_hosts: Arc<AllowList>,
    ) -> Result<Self, TakoError> {
        let mut builder = reqwest::Client::builder().timeout(timeout);
        if block_private_ips {
            builder = builder.dns_resolver(Arc::new(BlocklistResolver {
                allow_hosts: allow_hosts.clone(),
            }));
        }
        let http = builder.build().map_err(|e| {
            TakoError::Invalid(format!("ollama: failed to build prefetch client: {e}"))
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
                    let data_b64 = STANDARD.encode(&bytes);
                    *part = ContentPart::Image { mime, data_b64 };
                }
            }
        }
        Ok(())
    }

    async fn fetch_one(&self, url: &str) -> Result<(String, Vec<u8>), TakoError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| TakoError::Invalid(format!("ollama: prefetch URL parse failed: {e}")))?;
        let scheme = parsed.scheme();
        let scheme_ok = scheme == "https" || (self.allow_http && scheme == "http");
        if !scheme_ok {
            return Err(TakoError::Invalid(format!(
                "ollama: prefetch URL scheme `{scheme}` rejected (only `https://` allowed; \
                 use `with_url_prefetch_allow_http` to allow `http://`)",
            )));
        }

        // Phase 29.B — inline IP-literal check (mirror Phase 29.A).
        // reqwest's DNS resolver is not consulted for IP-literal
        // URLs, so the blocklist must be enforced here too.
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
                            "ollama: prefetch URL `{url}` resolves to blocked IP `{ip}` \
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
            .map_err(|e| TakoError::Transport(format!("ollama: prefetch GET {url}: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(TakoError::Invalid(format!(
                "ollama: prefetch GET {url} returned {status}: {body}",
            )));
        }

        if let Some(content_length) = resp.content_length() {
            let len_usize = usize::try_from(content_length).unwrap_or(usize::MAX);
            if len_usize > self.max_bytes {
                return Err(TakoError::Invalid(format!(
                    "ollama: prefetch response Content-Length {len_usize} exceeds max {}",
                    self.max_bytes,
                )));
            }
        }

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
        if !is_supported_ollama_mime(&content_type) {
            return Err(TakoError::Invalid(format!(
                "ollama: prefetch Content-Type `{content_type}` not supported \
                 (need one of image/jpeg, image/png, image/gif, image/webp)",
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| TakoError::Transport(format!("ollama: prefetch read body: {e}")))?;

        if bytes.len() > self.max_bytes {
            return Err(TakoError::Invalid(format!(
                "ollama: prefetch response body {} bytes exceeds max {}",
                bytes.len(),
                self.max_bytes,
            )));
        }

        Ok((content_type, bytes.to_vec()))
    }
}

/// Phase 28.B — accept the four common image MIMEs. Ollama
/// itself is more permissive (LLaVA-family models accept bytes
/// regardless of declared MIME) but we conservatively match the
/// other provider adapters.
fn is_supported_ollama_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Phase 29.B — return `true` for IPs that should be blocked
/// (loopback / private / link-local / multicast / reserved /
/// IPv6 equivalents). Mirror of `tako_providers_bedrock`'s
/// `is_blocked_ip` (per-crate copy per ARCHITECTURE.md hard rule
/// — provider crates depend only on `tako-core` + their vendor
/// SDK + reqwest; never on each other).
pub(crate) fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                // Multicast (224/4) + reserved (240/4 except broadcast).
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

/// Phase 29.B — `reqwest::dns::Resolve` impl that wraps
/// `tokio::net::lookup_host` and rejects any resolution where
/// ANY returned `SocketAddr` fails [`is_blocked_ip`]. Mirror of
/// the Bedrock crate's `BlocklistResolver`.
///
/// Phase 30 / 31 — `allow_hosts` is the per-host bypass. When
/// the requested hostname is matched by [`AllowList::contains`]
/// (exact-string for Phase 30 entries, wildcard suffix for
/// Phase 31 entries), the blocklist is skipped for that host
/// only.
#[derive(Debug)]
struct BlocklistResolver {
    allow_hosts: Arc<AllowList>,
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

/// Phase 31.B — split exact-match hostnames from wildcard
/// suffix patterns at config time. Mirror of
/// [`tako_providers_bedrock`]'s `AllowList` (per-crate copy per
/// ARCHITECTURE.md hard rule — provider crates depend only on
/// `tako-core` + their vendor SDK + reqwest; never on each
/// other).
///
/// Wildcard semantic: an entry `*.X` matches any host `Y` such
/// that `Y.ends_with(".X")` literally — multi-level subdomains
/// included.
#[derive(Debug, Default)]
pub(crate) struct AllowList {
    exact: HashSet<String>,
    suffixes: Vec<String>,
}

impl AllowList {
    pub(crate) fn from_strings(entries: Vec<String>) -> Self {
        let mut exact = HashSet::new();
        let mut suffixes = Vec::new();
        for entry in entries {
            if let Some(suffix) = entry.strip_prefix("*.") {
                suffixes.push(format!(".{suffix}"));
            } else {
                exact.insert(entry);
            }
        }
        Self { exact, suffixes }
    }

    pub(crate) fn contains(&self, host: &str) -> bool {
        self.exact.contains(host) || self.suffixes.iter().any(|s| host.ends_with(s.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.suffixes.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.exact.len() + self.suffixes.len()
    }
}

/// Phase 28.B — builder-side knobs collected on [`OllamaBuilder`].
#[derive(Debug, Clone)]
pub(crate) struct UrlPrefetchOpts {
    pub(crate) enabled: bool,
    pub(crate) allow_http: bool,
    pub(crate) timeout: Option<Duration>,
    pub(crate) max_bytes: Option<usize>,
    /// Phase 29.B — default `true` (block private / loopback /
    /// link-local IPs). Operators flip to `false` via
    /// [`OllamaBuilder::with_url_prefetch_allow_private_ips`] when
    /// deployment-level egress filtering is already enforced.
    pub(crate) block_private_ips: bool,
    /// Phase 30 — host strings that bypass the private-IP
    /// blocklist. Default empty. Populated via
    /// [`OllamaBuilder::with_url_prefetch_allow_host`].
    pub(crate) allow_hosts: Vec<String>,
}

impl Default for UrlPrefetchOpts {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_http: false,
            timeout: None,
            max_bytes: None,
            // Phase 29.B — default-deny stance for SSRF.
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
        let allow_hosts = Arc::new(AllowList::from_strings(self.allow_hosts));
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
            assert!(is_supported_ollama_mime(ok));
        }
        for bad in ["image/svg+xml", "text/plain", ""] {
            assert!(!is_supported_ollama_mime(bad));
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
            true,
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            // Phase 29.B — wiremock binds to 127.0.0.1, which the
            // default blocklist would reject. Disable for this test;
            // dedicated DNS-blocklist tests live below.
            false,
            // Phase 30 — empty allowlist for this test.
            Arc::new(AllowList::default()),
        )
        .unwrap();
        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        cfg.rewrite(&mut req).await.unwrap();

        let part = &req.messages[0].content[0];
        match part {
            ContentPart::Image { mime, data_b64 } => {
                assert_eq!(mime, "image/png");
                let decoded = STANDARD.decode(data_b64).unwrap();
                assert_eq!(decoded, TINY_PNG);
            }
            other => panic!("expected ContentPart::Image, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rewrite_rejects_http_url_by_default() {
        let cfg = UrlPrefetchConfig::new(
            false,
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            false,
            Arc::new(AllowList::default()),
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
            Arc::new(AllowList::default()),
        )
        .unwrap();
        let mut req = req_with_image_url(&format!("{}/icon.svg", server.uri()));
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("image/svg+xml"), "got: {msg}");
    }

    #[tokio::test]
    async fn rewrite_rejects_oversized_response_via_content_length() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/big.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(vec![0u8; 1024]),
            )
            .mount(&server)
            .await;

        let cfg = UrlPrefetchConfig::new(
            true,
            DEFAULT_TIMEOUT,
            100,
            false,
            Arc::new(AllowList::default()),
        )
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
            Arc::new(AllowList::default()),
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

    // ----- Phase 29.B — `is_blocked_ip` + DNS-resolver tests. -----

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
        let opts = UrlPrefetchOpts {
            enabled: true,
            block_private_ips: false,
            ..UrlPrefetchOpts::default()
        };
        let cfg = opts.into_config().unwrap();
        assert!(cfg.is_some());
    }

    /// Phase 29.B end-to-end: with the default-on blocklist, a
    /// pre-fetch request whose URL points at `127.0.0.1` (the
    /// wiremock loopback bind) must fail at IP-literal check,
    /// before any HTTP request is issued.
    #[tokio::test]
    async fn rewrite_rejects_resolved_loopback_ip_when_blocking() {
        let server = MockServer::start().await;
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
            true,
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            true,
            Arc::new(AllowList::default()),
        )
        .unwrap();

        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("blocked IP") || msg.contains("PermissionDenied"),
            "expected resolver rejection in error, got: {msg}",
        );
    }

    // ----- Phase 31 — `AllowList` wildcard semantics (mirror of Bedrock). -----

    #[test]
    fn allow_list_default_is_empty() {
        let allow = AllowList::default();
        assert!(allow.is_empty());
        assert_eq!(allow.len(), 0);
    }

    #[test]
    fn allow_list_exact_match() {
        let allow = AllowList::from_strings(vec!["registry.public.com".into(), "10.0.5.4".into()]);
        assert!(allow.contains("registry.public.com"));
        assert!(allow.contains("10.0.5.4"));
        assert!(!allow.contains("other.public.com"));
    }

    #[test]
    fn allow_list_wildcard_matches_subdomain() {
        let allow = AllowList::from_strings(vec!["*.internal.corp".into()]);
        assert!(allow.contains("registry.internal.corp"));
        assert!(allow.contains("images.internal.corp"));
    }

    #[test]
    fn allow_list_wildcard_matches_multi_level() {
        let allow = AllowList::from_strings(vec!["*.internal.corp".into()]);
        assert!(allow.contains("staging.images.internal.corp"));
        assert!(allow.contains("a.b.c.d.internal.corp"));
    }

    #[test]
    fn allow_list_wildcard_does_not_match_bare_domain() {
        let allow = AllowList::from_strings(vec!["*.internal.corp".into()]);
        assert!(!allow.contains("internal.corp"));
    }

    #[test]
    fn allow_list_wildcard_does_not_match_other_domain() {
        let allow = AllowList::from_strings(vec!["*.internal.corp".into()]);
        assert!(!allow.contains("evil.com"));
        assert!(!allow.contains("registry.public.com"));
    }

    #[test]
    fn allow_list_wildcard_does_not_match_attacker_domain() {
        // Confusable: `attacker-internal.corp` ends in
        // `internal.corp` but not `.internal.corp`.
        let allow = AllowList::from_strings(vec!["*.internal.corp".into()]);
        assert!(!allow.contains("attacker-internal.corp"));
    }

    #[test]
    fn allow_list_exact_and_wildcard_coexist() {
        let allow =
            AllowList::from_strings(vec!["registry.public.com".into(), "*.internal.corp".into()]);
        assert!(allow.contains("registry.public.com"));
        assert!(allow.contains("registry.internal.corp"));
        assert!(!allow.contains("evil.com"));
    }

    // ----- Phase 30 — per-host allowlist tests (mirror of Bedrock). -----

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

        let allow_hosts = Arc::new(AllowList::from_strings(vec!["127.0.0.1".into()]));

        let cfg =
            UrlPrefetchConfig::new(true, DEFAULT_TIMEOUT, DEFAULT_MAX_BYTES, true, allow_hosts)
                .unwrap();

        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        cfg.rewrite(&mut req).await.unwrap();

        match &req.messages[0].content[0] {
            ContentPart::Image { mime, data_b64 } => {
                assert_eq!(mime, "image/png");
                assert_eq!(STANDARD.decode(data_b64).unwrap(), TINY_PNG);
            }
            other => panic!("expected ContentPart::Image, got {other:?}"),
        }
    }

    /// Phase 30 — the allowlist matches the URL's host EXACTLY.
    /// A URL targeting `127.0.0.1` is NOT bypassed when only a
    /// different host is in the allowlist.
    #[tokio::test]
    async fn rewrite_does_not_allowlist_other_hosts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(TINY_PNG),
            )
            .expect(0)
            .mount(&server)
            .await;

        let allow_hosts = Arc::new(AllowList::from_strings(vec!["some-other-host".into()]));

        let cfg =
            UrlPrefetchConfig::new(true, DEFAULT_TIMEOUT, DEFAULT_MAX_BYTES, true, allow_hosts)
                .unwrap();

        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        let err = cfg.rewrite(&mut req).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("blocked IP") || msg.contains("PermissionDenied"),
            "expected blocklist rejection, got: {msg}",
        );
    }

    /// Phase 29.B regression pin: with the operator-opt-out flag,
    /// the same loopback URL succeeds.
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
            true,
            DEFAULT_TIMEOUT,
            DEFAULT_MAX_BYTES,
            false,
            Arc::new(AllowList::default()),
        )
        .unwrap();

        let mut req = req_with_image_url(&format!("{}/cat.png", server.uri()));
        cfg.rewrite(&mut req).await.unwrap();

        match &req.messages[0].content[0] {
            ContentPart::Image { mime, data_b64 } => {
                assert_eq!(mime, "image/png");
                assert_eq!(STANDARD.decode(data_b64).unwrap(), TINY_PNG);
            }
            other => panic!("expected ContentPart::Image, got {other:?}"),
        }
    }
}
