# PLAN — Phase 20 (Vision content support: Vertex + Mistral + Ollama)

## Context

Phase 19 (v0.20.0, 2026-05-01) wired outbound `ContentPart::Image`
through Anthropic and OpenAI — the two flagship providers. Bedrock
has shipped vision since Phase 2.5. That leaves three of the six
adapters still dropping image content: Vertex, Mistral, and Ollama.

Phase 20 finishes the vision-content sweep across the remaining
three providers. Each has a distinct wire format:

- **Vertex (Gemini)** uses an untagged `parts` array with an
  `inline_data` variant carrying `mime_type` + `data` (bare
  base64).
- **Mistral** is OpenAI-compatible: array-shaped `content` with
  `image_url` blocks holding a data-URL.
- **Ollama** is fundamentally different: a sibling
  `images: Vec<String>` field on `OlMessage` carrying bare base64
  strings. `content` stays a flat string.

Phase 19 reasoning carries over: Phase-1 byte-for-byte wire-shape
preservation on non-vision messages; same four supported MIME types
where the provider has a published list (`image/jpeg`, `image/png`,
`image/gif`, `image/webp`); silent drop on unsupported types;
data-URL prefix normalisation where applicable. Ollama is a partial
exception — the `images` field carries no MIME at all, so we strip
the data-URL prefix before sending and accept any MIME the caller
provided (the model decides what it can handle).

The Pydantic `ContentPart` mirror surface is unchanged from Phase
19 — Phase 19.C's facade smoke covers all six providers
identically. No new Python tests are required.

**Theme:** *Finish the vision-content sweep. After Phase 20 every
shipped provider adapter handles outbound image content.*

**Tag:** v0.21.0.

## A. Vertex outbound image content

### A.1 — Wire format

Vertex's Gemini API accepts `inline_data` parts on the same
`parts` array that already carries text + function calls:

```json
{
  "role": "user",
  "parts": [
    { "text": "describe this" },
    {
      "inline_data": {
        "mime_type": "image/jpeg",
        "data": "<base64-bytes>"
      }
    }
  ]
}
```

Note `inline_data` (snake_case), distinct from the camelCase
`functionCall` / `functionResponse` already in the enum.

### A.2 — Type extension

[`crates/tako-providers/vertex/src/convert.rs`](crates/tako-providers/vertex/src/convert.rs):

```rust
#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum VxPart {
    Text { text: String },
    FunctionCall { ... },
    FunctionResponse { ... },
    /// Phase 20.A — inline image content. Gemini also accepts
    /// `file_data` for cloud-stored URIs; we don't emit that yet
    /// (server-side fetch from request-supplied URLs has security
    /// implications, same reasoning as 19.A's `source.type = "url"`
    /// deferral).
    InlineData { inline_data: VxInlineData },
}

#[derive(Serialize, Debug)]
pub struct VxInlineData {
    pub mime_type: String,
    pub data: String,
}
```

The `InlineData` variant is named `inline_data` in JSON (matches
Gemini's snake_case field). Untagged enum + struct field naming
makes serde do the right thing without a `#[serde(rename)]`
attribute.

### A.3 — `message_to_vx` mapping

The existing `Image { .. } => { /* deferred */ }` arm becomes a
real mapping that filters MIME (same four types as Phase 19) and
strips the data-URL prefix before emission.

### A.4 — Tests

Three new unit tests in
[`vertex/src/convert.rs`](crates/tako-providers/vertex/src/convert.rs)
(or a new test module if the file has none — TBD on inspection):

1. `image_block_emits_inline_data_part` — pinned JSON shape.
2. `image_block_strips_data_url_prefix` — emitted `data` has no
   data-URL prefix.
3. `image_block_unsupported_mime_dropped` — silent drop.

## B. Mistral outbound image content

### B.1 — Wire format

Mistral's vision-capable models (Pixtral) accept OpenAI-compatible
content blocks:

```json
{
  "role": "user",
  "content": [
    { "type": "text", "text": "describe this" },
    {
      "type": "image_url",
      "image_url": "data:image/jpeg;base64,<base64>"
    }
  ]
}
```

Mistral accepts both the bare-string `image_url` form and the
nested `{"url": "..."}` form. We emit the nested form — it
matches OpenAI's adapter exactly (B.2 below) and Mistral's API
accepts it interchangeably.

### B.2 — Type extension

[`crates/tako-providers/mistral/src/convert.rs`](crates/tako-providers/mistral/src/convert.rs):

Same shape as Phase 19.B. New `MiContent` untagged enum and
`MiContentBlock` tagged enum. `MiMessage.content` field type
widens from `Option<String>` to `Option<MiContent>`.

### B.3 — `message_to_mi` refactor

Same pattern as Phase 19.B's `message_to_oa`:
- No image parts → `Some(MiContent::Text(joined))` —
  byte-for-byte parity with pre-20.B.
- At least one image part → `Some(MiContent::Blocks(blocks))`
  with text+image entries in source order.
- Tool-result messages keep the flat-string shape.

Same MIME filter (four types), same `build_data_url` /
`strip_data_url_prefix` helpers (identical to OpenAI's per-crate
copies; not lifted into a shared crate to keep provider
independence).

### B.4 — Tests

Five new unit tests mirroring 19.B's coverage:

1. `text_only_message_keeps_flat_string_content` — regression.
2. `image_block_emits_array_content_with_image_url`.
3. `image_block_normalises_data_url_prefix`.
4. `image_block_unsupported_mime_dropped`.
5. `tool_result_message_keeps_flat_string_content`.

## C. Ollama outbound image content

### C.1 — Wire format

Ollama's `/api/chat` endpoint accepts an `images` field on the
message — a sibling of `content`, not a content-block array:

```json
{
  "role": "user",
  "content": "describe this",
  "images": ["<base64-bytes>", "<base64-bytes>"]
}
```

Each entry is bare base64 (no data-URL prefix, no MIME). Ollama
runs the bytes through the model directly; the model decides what
formats it can decode.

### C.2 — Type extension

[`crates/tako-providers/ollama/src/convert.rs`](crates/tako-providers/ollama/src/convert.rs):

```rust
pub struct OlMessage {
    pub role: &'static str,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OlToolCall>,
    /// Phase 20.C — Ollama-specific sibling field. `Vec::is_empty`
    /// gates emission so non-vision messages keep byte-for-byte
    /// wire shape.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}
```

### C.3 — `message_to_ol` refactor

Walk `m.content`, partition into text + image content. `content`
field stays the joined text (or `tool_result` as before); `images`
field gets the base64 strings with data-URL prefixes stripped.

Ollama doesn't filter MIMEs — accept any MIME the caller provides
and let the model decide. The image-content MIME field is captured
internally but not transmitted to Ollama; it would only matter for
a future server-side validation step.

### C.4 — Tests

Four new unit tests:

1. `text_only_message_omits_images_field` — regression: no
   `images` key in the serialised JSON when no image parts
   present (`skip_serializing_if = "Vec::is_empty"` does its job).
2. `image_block_populates_images_field` — pinned JSON shape;
   `content` keeps its text, `images` carries base64.
3. `multiple_images_preserve_source_order` — two image content
   parts → two-element `images` array in source order.
4. `image_block_strips_data_url_prefix` — Ollama wants bare
   base64, not data-URL.

## D. Cleanup

The three "deferred to Phase 20+" stub markers go away with the
implementation:
- [`vertex/src/convert.rs:202-208`](crates/tako-providers/vertex/src/convert.rs#L202-L208)
- [`mistral/src/convert.rs:174-178`](crates/tako-providers/mistral/src/convert.rs#L174-L178)
- [`ollama/src/convert.rs:161-165`](crates/tako-providers/ollama/src/convert.rs#L161-L165)

After Phase 20 every shipped provider adapter (Anthropic, OpenAI,
Vertex, Bedrock, Mistral, Ollama) handles outbound image content.
The Phase-19 Python facade smoke
([`tests/python/test_phase19_vision.py`](tests/python/test_phase19_vision.py))
still applies — `ContentPart` is the same Pydantic surface — no
new Python tests required.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  `image_block_*` tests in 20.A.4 / 20.B.4 / 20.C.4 pass; existing
  Vertex / Mistral / Ollama integration tests still byte-for-byte
  green (regression: text-only messages must NOT change wire
  shape; Mistral specifically must keep the flat-string content
  shape).

## Out of scope (Phase 21+)

- **Inbound image responses** from any provider — image-generation
  APIs are a separate model class.
- **URL-source images** — Anthropic's `source.type = "url"`,
  OpenAI's `image_url.url` with `https://...`, Vertex's
  `file_data` with `file_uri`. Server-side fetch from
  request-supplied URLs needs a security story.
- **Mistral's bare-string `image_url` shorthand** — both forms
  serialise identically through Mistral's API; we always emit the
  nested form for OpenAI-adapter parity.
- OIDC introspection mTLS auth methods, OIDC refresh-token /
  revocation-endpoint flows, composite `AuthResolver`s.
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).

## Commits

1. `feat(tako-providers/vertex): outbound image content (Phase 20.A)`
2. `feat(tako-providers/mistral): outbound image content (Phase 20.B)`
3. `feat(tako-providers/ollama): outbound image content (Phase 20.C)`
4. `docs: Phase 20 PLAN/README/CHANGELOG flip (v0.21.0)`
