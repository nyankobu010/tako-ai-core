# PLAN — Phase 22 (URL-source images: Anthropic + OpenAI + Mistral)

## Context

Phase 19 (v0.20.0) wired outbound `ContentPart::Image` (base64
inline data) through Anthropic + OpenAI; Phase 20 (v0.21.0)
finished the sweep across Vertex + Mistral + Ollama. Both phases
explicitly deferred **URL-source** images per the same reasoning:
"server-side fetch from request-supplied URLs needs a security
story we haven't designed yet".

That security concern was framed too broadly. The relevant question
is *who fetches the URL*:

- **Anthropic / OpenAI / Mistral**: the *vendor's* API server
  fetches the URL. tako only passes the URL string through to
  the provider; no SSRF risk on tako's side. Same security
  posture as a direct-from-browser call to those vendors.
- **Vertex (Gemini)**: `fileData` accepts vendor-specific URIs
  (`gs://...` GCS, or Vertex's File API URIs). Not arbitrary
  `https://...`. Vendor-specific shape — defer to Phase 23+.
- **Bedrock**: AWS Bedrock's `ImageSource` accepts `Bytes` only;
  no URL variant in the SDK. Defer.
- **Ollama**: `images` field carries bare base64 only. Defer.

Phase 22 wires URL-source through the three vendors that accept
arbitrary `https://` URLs. The remaining three keep silent-drop
stubs.

**Theme:** *Close the URL-source-image gap on the three vendors
that fetch URLs themselves; defer vendor-specific URI shapes to
Phase 23+.*

**Tag:** v0.23.0.

## A. `tako-core::ContentPart::ImageUrl` variant + workspace stubs

### A.1 — Type extension

[`crates/tako-core/src/types.rs`](crates/tako-core/src/types.rs):

```rust
pub enum ContentPart {
    Text { text: String },
    Image { mime: String, data_b64: String },
    /// Phase 22 — URL-source image. The provider's API server
    /// fetches `url`; tako passes it through unchanged. `mime`
    /// is an optional hint (some vendors use it; others ignore
    /// it). Use for HTTPS URLs only — the security story for
    /// `http://` URLs and vendor-specific URIs (e.g. Vertex's
    /// `gs://...`) remains deferred to Phase 23+.
    ImageUrl { url: String, mime: Option<String> },
    ToolCall { ... },
    ToolResult { ... },
}
```

Strictly additive — pre-1.0; adding a variant is acceptable.
Existing `match`-without-wildcard sites need an arm; matches
with `_ =>` wildcards (most orchestrator / tako-py / tako-compat
sites) are unaffected.

### A.2 — Workspace match-site updates

The six provider `convert.rs` files have exhaustive `match c`
arms in their `message_to_*` mappers (Phases 19 + 20):

- `tako-providers/anthropic/src/convert.rs:174-209` — wire to
  `AnImageSource::Url { url }` (full implementation in B).
- `tako-providers/openai/src/convert.rs:198-228` — wire to
  `OaImageUrl { url }` (full implementation in C).
- `tako-providers/mistral/src/convert.rs:201-231` — wire to
  `MiImageUrl { url }` (full implementation in C).
- `tako-providers/vertex/src/convert.rs:191-237` — silent-drop
  with deferred-to-Phase-23+ comment (Gemini's `fileData`
  accepts only `gs://...` URIs).
- `tako-providers/bedrock/src/convert.rs:140-213` — silent-drop
  with deferred-to-Phase-23+ comment (Bedrock SDK's `ImageSource`
  has no URL variant).
- `tako-providers/ollama/src/convert.rs:163-180` — silent-drop
  with deferred-to-Phase-23+ comment (Ollama's `images` field
  carries bare base64 only).

Phase 22.A's commit lands the core variant + the three
silent-drop stubs (Vertex / Bedrock / Ollama). The three
implementing adapters get their full wiring in 22.B (Anthropic)
and 22.C (OpenAI + Mistral); they keep a placeholder
silent-drop arm in 22.A's commit so each commit compiles
standalone.

## B. Anthropic URL-source

### B.1 — `AnImageSource` struct → enum

Phase 19.A's `AnImageSource` is currently a struct holding only
the base64-source case:

```rust
pub struct AnImageSource {
    pub kind: &'static str,  // always "base64"
    pub media_type: String,
    pub data: String,
}
```

Refactor to a `#[serde(tag = "type")]`-tagged enum with two
variants. Phase 19.A's wire shape on the `Base64` variant is
identical (the `kind: "base64"` discriminator becomes
`#[serde(rename = "base64")]` from the enum tag).

```rust
#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnImageSource {
    Base64 { media_type: String, data: String },
    /// Phase 22 — URL-source. Anthropic's API server fetches
    /// `url`. Per Anthropic Messages API: `{"type": "url", "url":
    /// "https://..."}`.
    Url { url: String },
}
```

Public-API impact: `AnImageSource` was a public struct; it's now
a public enum. The existing `Image { source: AnImageSource }`
variant on `AnBlock` is unchanged shape.

### B.2 — Mapping

[`anthropic/src/convert.rs`](crates/tako-providers/anthropic/src/convert.rs):

```rust
ContentPart::ImageUrl { url, mime: _ } => {
    Some(AnBlock::Image {
        source: AnImageSource::Url { url: url.clone() },
    })
}
```

Anthropic's URL-source variant takes no `media_type` field
(per the published API); the optional `mime` from the core
type is intentionally dropped. Phase 22.B does not pre-validate
the URL scheme — Anthropic's API rejects non-`https://` URLs at
the API boundary, which is the right error surface (matches
their own validation).

### B.3 — Tests

Three new unit tests in
[`anthropic/src/convert.rs`](crates/tako-providers/anthropic/src/convert.rs):

1. `image_url_block_emits_url_source` — `ContentPart::ImageUrl
   { url: "https://example.com/cat.jpg", mime: None }` →
   serialised JSON contains `{"type": "url", "url":
   "https://example.com/cat.jpg"}`.
2. `image_url_block_drops_mime_hint` — `mime: Some("image/png")`
   on the core type → emitted source has no `media_type` /
   `mime` field (Anthropic's URL variant doesn't accept one).
3. `image_block_base64_wire_shape_unchanged` — regression pin
   that the Phase 19.A base64 path serialises byte-for-byte
   identically after the struct→enum refactor.

## C. OpenAI + Mistral URL-source

### C.1 — Mapping

OpenAI accepts `https://` URLs in `image_url.url` directly (the
field is the same one that holds data-URLs today). The adapter
just stops wrapping URLs in `data:...` prefixes when the input
is already a URL.

[`openai/src/convert.rs`](crates/tako-providers/openai/src/convert.rs):

```rust
ContentPart::ImageUrl { url, mime: _ } => {
    has_image = true;
    blocks.push(OaContentBlock::ImageUrl {
        image_url: OaImageUrl { url: url.clone() },
    });
}
```

[`mistral/src/convert.rs`](crates/tako-providers/mistral/src/convert.rs):
identical shape.

Both: the URL passes through unchanged. No MIME filtering — the
vendor decides what URLs it can fetch and what content types it
accepts; URL-source images don't need a MIME hint. The optional
`mime` on the core type is intentionally dropped (matches
Anthropic's choice in B.2).

### C.2 — Tests

Two new unit tests per provider (four total):

1. `image_url_block_emits_array_content_with_url` — pinned JSON
   shape: `image_url.url` carries the `https://...` string
   verbatim.
2. `image_url_does_not_get_data_url_wrapped` — regression pin
   that we don't accidentally wrap an `https://...` URL in
   `data:...;base64,` (which would break the URL).

## D. Python facade

### D.1 — Pydantic `ContentPart` field

[`python/tako/models.py`](python/tako/models.py): add an explicit
`url: str | None = None` field to `ContentPart`. The Pydantic
model already has `extra="allow"` so unrecognised `type="image_url"`
+ `url=...` would round-trip, but adding the explicit field gives
better type checking / IDE completion / error messages.

### D.2 — Tests

Four new tests in
[`tests/python/test_phase22_image_url.py`](tests/python/test_phase22_image_url.py):

1. `test_content_part_accepts_image_url_variant` — construct
   `ContentPart(type="image_url", url="https://...", mime=None)`.
2. `test_content_part_image_url_serialises_to_expected_dict` —
   `model_dump(exclude_none=True, exclude_defaults=True)` yields
   the wire-shape the Rust adapters consume.
3. `test_message_can_carry_mixed_text_and_image_url` — source
   order preservation.
4. `test_content_part_image_url_with_optional_mime_hint` — mime
   as Some.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  `image_url_*` tests in 22.B / 22.C pass; existing 19 / 20
  base64 wire-shape tests still byte-for-byte green; non-vision
  message regression pins (`text_only_message_keeps_flat_string_content`
  in OpenAI / Mistral, `text_only_message_omits_images_field` in
  Ollama) still green.
- `pytest -q tests/python/test_phase22_image_url.py` — green.

## Out of scope (Phase 23+)

- **Vertex `fileData` URL source** — Gemini accepts only
  vendor-specific URIs (GCS `gs://...` or the Vertex File API
  URI scheme). Different shape from arbitrary `https://`. Needs
  a per-URL-scheme branch that recognises `gs://` etc.
- **Bedrock URL source** — the AWS SDK's `ImageSource` has no
  URL variant; would require pre-fetching the bytes server-side
  (back to the SSRF question that Phase 22 explicitly dodged by
  doing vendor-fetched URLs only).
- **Ollama URL source** — Ollama's `images` field carries bare
  base64; needs the same pre-fetch.
- **`http://` (non-TLS) URL hardening** — Phase 22 doesn't
  enforce `https://`; Anthropic / OpenAI / Mistral all reject
  non-`https` URLs at their own API boundary. A future phase
  may add tako-side pre-validation.
- mTLS / refresh-token / eval-graders / `ChainedAuthResolver`
  short-circuit semantics carried over from Phase 21.

## Commits

1. `feat(tako-core): ContentPart::ImageUrl variant + provider stubs (Phase 22.A)`
2. `feat(tako-providers/anthropic): URL-source images via AnImageSource enum (Phase 22.B)`
3. `feat(tako-providers/openai+mistral): URL-source images pass-through (Phase 22.C)`
4. `feat(tako-py): ContentPart.url Python facade (Phase 22.D)`
5. `docs: Phase 22 PLAN/README/CHANGELOG flip (v0.23.0)`
