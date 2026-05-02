# PLAN — Phase 28 (URL-source images: Bedrock + Ollama via opt-in tako-side pre-fetch)

## Context

Phases 22 + 23 wired URL-source images through the four providers
whose API servers fetch URLs themselves: Anthropic, OpenAI,
Mistral (Phase 22), and Vertex/Gemini (Phase 23). Each one gets
a URL pass-through — tako serialises the URL into the wire
format; the vendor's API server fetches.

Bedrock and Ollama have a fundamentally different model. Per
Phase 22.A's framing:
- **Bedrock** — the AWS Bedrock SDK's `ImageSource` exposes only
  `Bytes`; there's no URL variant. URL-source via Bedrock would
  require tako-side pre-fetch.
- **Ollama** — the `images` field carries bare base64 only;
  same pre-fetch concern.

Both got silent-drop stubs in Phase 22.A with the framing
"deferred to Phase 23+ pending an SSRF security design".

Phase 28 ships the SSRF design + opt-in pre-fetch for both.
After Phase 28, every shipped provider adapter (Anthropic,
OpenAI, Vertex, Bedrock, Mistral, Ollama — six of six) handles
both inline-base64 (Phase 19/20) AND URL-source images (Phase
22/23 pass-through, Phase 28 pre-fetch). The vision-content arc
is closed.

## SSRF design

Tako-side URL fetch raises SSRF risk: an attacker who can
inject a `ContentPart::ImageUrl` into a request can ask tako to
fetch arbitrary URLs. Mitigations:

1. **Opt-in.** `with_url_prefetch()` on the provider builder.
   Default is silent-drop (Phase 22.A semantics preserved).
   Operators must explicitly enable.
2. **`https://`-only by default.** Reject `http://` and other
   schemes (`gs://`, `file://`, etc.) at the URL parse stage.
   Operators with internal HTTP-only artifact servers can opt
   in to `http://` via a separate builder
   (`with_url_prefetch_allow_http()`).
3. **Connect+read timeout.** 10 seconds. Configurable via
   `with_url_prefetch_timeout(Duration)`.
4. **Response size cap.** 10 MB. Enforced via `Content-Length`
   header check (when present) + post-fetch byte-length check.
   Configurable via `with_url_prefetch_max_bytes(usize)`.
5. **MIME validation.** `Content-Type` must be one of
   `image/{jpeg,png,gif,webp}` (matches what each provider
   accepts upstream).

Out of scope for Phase 28: CIDR blocklist for private/
link-local IPs, DNS-rebinding mitigation. Operators must
enforce network egress policy at deployment level (via VPC
egress rules, Pod-level egress NetworkPolicies, etc.). Phase
29+ may add per-request CIDR check + resolve-once-then-connect.

## A. Bedrock URL pre-fetch

### A.1 — `url_prefetch` module

[`crates/tako-providers/bedrock/src/url_prefetch.rs`](crates/tako-providers/bedrock/src/url_prefetch.rs)
(new file):

```rust
pub struct UrlPrefetchConfig {
    pub allow_http: bool,
    pub timeout: Duration,
    pub max_bytes: usize,
    pub http: reqwest::Client, // pre-built per-provider client
}

impl UrlPrefetchConfig {
    pub fn new() -> Result<Self, TakoError> { /* ... */ }

    /// Walk `req.messages`, fetch each `ContentPart::ImageUrl`
    /// in place, rewrite to `ContentPart::Image { mime, data_b64 }`.
    /// Errors short-circuit the whole request.
    pub async fn rewrite(&self, req: &mut ChatRequest) -> Result<(), TakoError> { /* ... */ }
}
```

### A.2 — `BedrockBuilder` extension

```rust
impl BedrockBuilder {
    /// Phase 28 — opt in to tako-side pre-fetch for
    /// `ContentPart::ImageUrl` content. Default is silent-drop.
    pub fn with_url_prefetch(mut self) -> Self { ... }

    pub fn with_url_prefetch_allow_http(mut self) -> Self { ... }

    pub fn with_url_prefetch_timeout(mut self, d: Duration) -> Self { ... }

    pub fn with_url_prefetch_max_bytes(mut self, n: usize) -> Self { ... }
}
```

`Inner` gains `url_prefetch: Option<UrlPrefetchConfig>`.

### A.3 — `chat()` / `stream()` pre-pass

```rust
async fn chat(&self, p: &Principal, mut req: ChatRequest) -> Result<...> {
    if let Some(cfg) = &self.inner.url_prefetch {
        cfg.rewrite(&mut req).await?;
    }
    // ... existing logic
}
```

Same insertion point in `stream()`.

### A.4 — Tests

Six new integration tests in
[`crates/tako-providers/bedrock/tests/url_prefetch.rs`](crates/tako-providers/bedrock/tests/url_prefetch.rs)
(new file) using wiremock to serve fixture images:

1. `prefetch_disabled_default_drops_image_url` — Phase 22.A
   regression: without `with_url_prefetch()`, `ImageUrl` is
   silently dropped.
2. `prefetch_enabled_fetches_image_and_emits_inline_base64` —
   wiremock serves a 1×1 PNG; tako fetches; the rewritten
   `ContentPart::Image` carries the correct bytes.
3. `prefetch_rejects_http_url_by_default` — `http://...` URL →
   `TakoError::Invalid` (without `allow_http`).
4. `prefetch_allow_http_accepts_http_url` — opt-in to HTTP for
   internal artifact servers.
5. `prefetch_rejects_oversized_response` — wiremock serves a
   response that exceeds the size cap → `TakoError::Invalid`.
6. `prefetch_rejects_unsupported_mime` — wiremock serves a
   `text/plain` response → `TakoError::Invalid`.

## B. Ollama URL pre-fetch

Same shape as 28.A: `url_prefetch.rs` module +
`OllamaBuilder::with_url_prefetch*` methods + chat/stream
pre-pass + tests.

The pre-fetch helper is per-crate (ARCHITECTURE.md hard rule
forbids cross-provider deps), so the design choices are
copy-pasted across the two crates with provider-specific tweaks
where needed (e.g. Bedrock's MIME → `aws_sdk_bedrockruntime::ImageFormat`
mapping, Ollama's bare-base64 sibling field).

## C. Python facade

[`crates/tako-py/src/py_bedrock.rs`](crates/tako-py/src/py_bedrock.rs)
+ corresponding Ollama binding (TBD on inspection): builder
gains keyword args `url_prefetch=False`,
`url_prefetch_allow_http=False`, `url_prefetch_timeout_secs=10`,
`url_prefetch_max_bytes=10 * 1024 * 1024`.

[`tests/python/test_phase28_url_prefetch.py`](tests/python/test_phase28_url_prefetch.py):
facade attribute presence + parameter wiring smoke. Live tests
remain on the Rust side.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  `url_prefetch_*` tests in 28.A.4 + 28.B pass; existing
  Bedrock + Ollama tests still byte-for-byte green (regression:
  the Phase 22.A silent-drop default must NOT change).
- `pytest -q tests/python/test_phase28_url_prefetch.py` — green.

## Out of scope (Phase 29+)

- **CIDR blocklist** (private / link-local / loopback IPs).
  Needs DNS-resolve-once-then-connect to mitigate DNS
  rebinding. ~150 lines of additional security logic.
- **Vertex File API upload flow** — separate API surface for
  uploading bytes to Vertex's File API and getting back a
  Vertex File URI. Needs a separate builder + API.
- **OIDC mTLS end-to-end integration test, mTLS cert rotation,
  eval-harness real graders, OIDC refresh-token / revocation,
  per-child ChainedAuthResolver policy override** — all carried
  over.

## Commits

1. `feat(tako-providers/bedrock): URL-source image pre-fetch (Phase 28.A)`
2. `feat(tako-providers/ollama): URL-source image pre-fetch (Phase 28.B)`
3. `feat(tako-py): Bedrock + Ollama url_prefetch facade (Phase 28.C)`
4. `docs: Phase 28 PLAN/README/CHANGELOG flip (v0.29.0)`
