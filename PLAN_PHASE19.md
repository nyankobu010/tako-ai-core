# PLAN — Phase 19 (Vision content support: Anthropic + OpenAI)

## Context

Phase 18 (v0.19.0, 2026-05-01) closed the OIDC introspection
auth-method surface — `private_key_jwt` (RFC 7521 / 7523, RS256 /
ES256 / EdDSA) and an OIDC Session Management 1.0 end-session
helper. After five consecutive auth-hardening phases (15.B, 16.B,
17, 18) it's time to step out of the auth path.

[`PLAN.md`](PLAN.md) lines 49–63 list the Phase-19 carry-forward.
**Vision / image content support** is the long-deferred top-line
item: `tako_core::ContentPart::Image { mime, data_b64 }` has shipped
since Phase 1, and Bedrock has wired it since Phase 2.5, but the
two flagship providers (Anthropic and OpenAI) still discard image
content blocks with explicit "out of scope for Phase 1" markers
(see [`anthropic/src/convert.rs:171`](crates/tako-providers/anthropic/src/convert.rs#L171)
and [`openai/src/convert.rs:169-173`](crates/tako-providers/openai/src/convert.rs#L169-L173)).

Phase 19 wires vision through Anthropic and OpenAI. Vertex and
Mistral remain stub markers, deferred to Phase 20+ — a single
phase that touches Anthropic + OpenAI is already a meaningful
chunk of work, and the wire formats are different enough across
the four providers that bundling all of them inflates risk.

**Theme:** *Surface the long-shipped `ContentPart::Image` through
the two flagship providers.*

**Tag:** v0.20.0.

## A. Anthropic outbound image content

### A.1 — Wire format

Anthropic's Messages API accepts image content blocks in user-role
messages with a base64 source:

```json
{
  "type": "image",
  "source": {
    "type": "base64",
    "media_type": "image/jpeg",
    "data": "<base64-bytes>"
  }
}
```

Supported media types: `image/jpeg`, `image/png`, `image/gif`,
`image/webp`. Other types are rejected by the API.

### A.2 — Type extension

[`crates/tako-providers/anthropic/src/convert.rs`](crates/tako-providers/anthropic/src/convert.rs):

```rust
#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnBlock {
    Text { text: String },
    ToolUse { ... },
    ToolResult { ... },
    /// Phase 19.A — vision content. Anthropic accepts only the
    /// base64-source variant here; URL sources require the
    /// `image/url` source type which we don't emit yet.
    Image { source: AnImageSource },
}

#[derive(Serialize, Debug)]
pub struct AnImageSource {
    #[serde(rename = "type")]
    pub kind: &'static str,  // always "base64" in Phase 19
    pub media_type: String,
    pub data: String,
}
```

### A.3 — `content_to_blocks` mapping

The existing `Image { .. } => None` arm becomes:

```rust
ContentPart::Image { mime, data_b64 } => {
    if !is_supported_anthropic_mime(mime) {
        return None;  // silently drop unsupported types
    }
    Some(AnBlock::Image {
        source: AnImageSource {
            kind: "base64",
            media_type: mime.clone(),
            data: strip_data_url_prefix(data_b64).to_string(),
        },
    })
}
```

Two helpers ship in this commit:
- `is_supported_anthropic_mime(s) -> bool` — accepts the four
  Anthropic-supported types listed in A.1.
- `strip_data_url_prefix(s) -> &str` — strips a leading
  `data:image/...;base64,` prefix when present, returns `s`
  unchanged otherwise. Idempotent.

The "silently drop unsupported types" choice (rather than erroring)
matches the existing `Text { text: "" } => None` cadence in the
same function — invalid content is swallowed, not surfaced. A
future phase may surface this through a `tracing::warn!` log at
the `ContentPart` boundary.

### A.4 — Tests

Three new unit tests in
[`anthropic/src/convert.rs`](crates/tako-providers/anthropic/src/convert.rs):

1. `image_block_emits_base64_source` — `ContentPart::Image { mime:
   "image/png", data_b64: "<...>" }` → `AnBlock::Image` with the
   correct `source` shape; serde-serialise the result and pin the
   JSON.
2. `image_block_strips_data_url_prefix` — input
   `data_b64="data:image/jpeg;base64,abc"` → emitted `data="abc"`.
3. `image_block_unsupported_mime_dropped` — `mime="image/svg+xml"`
   → no `AnBlock` emitted (filter_map drops it).

## B. OpenAI outbound image content

### B.1 — Wire format

OpenAI's Chat Completions API requires the `content` field of a
message to switch from a flat string to an array of typed blocks
when an image is present:

```json
{
  "role": "user",
  "content": [
    { "type": "text", "text": "describe this image" },
    {
      "type": "image_url",
      "image_url": { "url": "data:image/jpeg;base64,<base64>" }
    }
  ]
}
```

Plain-text messages MAY still be emitted as a flat string — the
API accepts both shapes. To preserve byte-for-byte wire shape on
existing non-vision traffic, the adapter emits the array form
**only when** an image content part is present.

### B.2 — Type extension

[`crates/tako-providers/openai/src/convert.rs`](crates/tako-providers/openai/src/convert.rs):

```rust
#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum OaContent {
    /// Phase 1 default — flat string shape.
    Text(String),
    /// Phase 19.B — array of typed blocks (required when an
    /// `image_url` block is present).
    Blocks(Vec<OaContentBlock>),
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OaContentBlock {
    Text { text: String },
    ImageUrl { image_url: OaImageUrl },
}

#[derive(Serialize, Debug)]
pub struct OaImageUrl {
    pub url: String,
}
```

`OaMessage.content` changes type from `Option<String>` to
`Option<OaContent>`. The `#[serde(skip_serializing_if = "Option::is_none")]`
attribute keeps emitting `null` content as a missing field.

### B.3 — `message_to_oa` refactor

Existing logic accumulates `text_parts: Vec<&str>` then joins
them into a single string. The refactor walks once, partitions
into `text_parts` + `image_parts`, then chooses:

- **No image parts:** emit `Some(OaContent::Text(joined))` —
  byte-for-byte parity with Phase 1.
- **At least one image part:** emit
  `Some(OaContent::Blocks(blocks))` where `blocks` interleaves
  text and image entries in the order they appeared in the
  source `Message.content`. Preserves narrative order ("here's
  the question, then here's the image").

The `image_url.url` value is constructed as:

```
data:<mime>;base64,<data_b64-with-prefix-stripped>
```

— i.e. the adapter normalises to a canonical data-URL even when
the caller supplied either form. Same `strip_data_url_prefix`
helper as A.3, lifted into a shared helper module.

Unsupported mime types (anything not matching `image/{jpeg,png,gif,webp}`)
are silently dropped, matching A.3's choice.

### B.4 — Tests

Three new unit tests:

1. `image_block_emits_array_content_with_image_url` — text +
   image content → array shape with two blocks in order; pin the
   serialised JSON.
2. `text_only_message_keeps_flat_string_content` — non-vision
   message → `OaContent::Text("...")` shape (regression on the
   byte-for-byte wire shape preservation).
3. `image_block_normalises_data_url_prefix` — input with bare
   base64 (no prefix) and input with `data:...,` prefix both
   yield the same canonical data-URL form in the request.

## C. Python facade smoke test

`tako_core::ContentPart::Image` already round-trips through
[`crates/tako-py/src/py_message.rs`](crates/tako-py/src/py_message.rs)
(or wherever the `Message` / `ContentPart` PyO3 bindings live —
TBD on inspection) since Phase 1. Phase 19 doesn't change the
PyO3 surface; the facade smoke test confirms an end-to-end
`tako.providers.Anthropic`-style construction with an image
content part doesn't raise, AND that the request body the wheel
emits to a wiremock'd Anthropic endpoint contains the expected
image block.

If the Python `ContentPart` constructor doesn't already accept
an `Image` kwarg, that's a separate Phase 19.D fix; otherwise
the facade smoke is purely additive.

`tests/python/test_phase19_vision.py` covers:
- `ContentPart`-equivalent dict shapes the Python facade emits
  serialise correctly through the wheel.
- A `tako.SingleAgent.run` against a wiremock'd Anthropic
  endpoint with an image-bearing user message sees the expected
  outbound body (`{"type": "image", "source": ...}`).

(If wiremocking through PyO3 is awkward, fallback: just assert
the type surface — `ContentPart` accepts `Image` shape — and let
the Rust unit tests be the source of truth.)

## D. Cleanup

Two stale `// vision is out of scope for Phase 1` markers go
away with the implementation:
- [`anthropic/src/convert.rs:171`](crates/tako-providers/anthropic/src/convert.rs#L171)
- [`openai/src/convert.rs:169-173`](crates/tako-providers/openai/src/convert.rs#L169-L173)

Mistral and Vertex keep their stub markers (deferred to Phase 20+);
their stub comments are amended to reference Phase 20+ rather
than "Phase 1".

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  `image_block_*` tests in 19.A.4 / 19.B.4 pass; existing
  Anthropic and OpenAI adapter tests still byte-for-byte green
  (regression: text-only messages must NOT change wire shape).
- `pytest -q tests/python/test_phase19_vision.py` — green.

## Out of scope (Phase 20+)

- **Vertex** image content (`convert.rs:202`) — uses Google's
  `inline_data` / `file_data` part shapes; requires per-part
  inspection of the `gemini-pro-vision` SDK surface.
- **Mistral** image content (`convert.rs:174`) — Mistral's
  multimodal API support is newer / model-specific.
- **Inbound image responses** from any provider — no provider
  currently ships images-as-output in the standard text-completion
  endpoints we adapt; this only matters for image-generation APIs
  which are a separate model class.
- **URL-source images** (Anthropic's `source.type = "url"` /
  OpenAI's `image_url.url` with a `https://...` value) — Phase 19
  emits only base64 sources. URL sources need a fetch-and-decode
  story we haven't designed yet (security implications around
  fetching arbitrary URLs server-side from a tako request).
- **mTLS / refresh-token / composite-AuthResolver** OIDC items
  carried forward from Phase 18 stay deferred.
- Eval harness real graders, OTel end-to-end real-collector test.

## Commits

1. `feat(tako-providers/anthropic): outbound image content (Phase 19.A)`
2. `feat(tako-providers/openai): outbound image content (Phase 19.B)`
3. `test(tako-py): vision content Python facade smoke (Phase 19.C)`
4. `docs: Phase 19 PLAN/README/CHANGELOG flip (v0.20.0)`
