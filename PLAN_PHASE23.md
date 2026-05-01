# PLAN — Phase 23 (URL-source images for Vertex / Gemini fileData)

## Context

Phase 22 (v0.23.0, 2026-05-01) wired URL-source images through
Anthropic + OpenAI + Mistral — the three vendors whose API
servers fetch URLs themselves. Vertex was deferred with the
framing "Gemini's `fileData` accepts only vendor-specific URI
schemes (`gs://...` GCS, Vertex File API URIs); not arbitrary
`https://`".

That framing was incomplete. Per Gemini's published API docs,
`fileData` accepts URIs from three sources:

1. **Google Cloud Storage** (`gs://bucket/path`) — Google
   fetches server-side; private buckets need IAM auth on
   Google's side, not tako's.
2. **Public web URLs** (`https://example.com/image.jpg`) —
   Gemini's API server fetches the URL directly, identical
   security posture to Anthropic / OpenAI / Mistral.
3. **Vertex File API URIs** — files uploaded to Google's File
   API and referenced by their URI; out of scope for Phase 23
   because tako doesn't expose a File-API upload surface.

Phase 23 covers (1) and (2). The Vertex File API upload flow
(3) and Bedrock / Ollama URL-source (which would need tako-side
pre-fetch with an SSRF guard) remain deferred.

`fileData` requires a `mimeType` field. The optional `mime` on
`ContentPart::ImageUrl` is required for the Vertex path —
without it, we silently drop the part (consistent with the
empty-text drop policy elsewhere in the adapter).

**Theme:** *Extend Phase 22's URL-source-image work to Vertex,
the fourth vendor whose API server fetches URLs.*

**Tag:** v0.24.0.

## A. Vertex `VxPart::FileData` variant + URL-source mapping

### A.1 — Type extension

[`crates/tako-providers/vertex/src/convert.rs`](crates/tako-providers/vertex/src/convert.rs):

```rust
#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum VxPart {
    Text { text: String },
    FunctionCall { ... },
    FunctionResponse { ... },
    InlineData { inline_data: VxInlineData },
    /// Phase 23 — URL-source image. Gemini's `fileData` part
    /// accepts URIs that Google's API server fetches:
    /// - `gs://bucket/path` Google Cloud Storage URIs
    /// - `https://...` public web URLs
    /// - Vertex File API URIs (out of scope for Phase 23)
    ///
    /// Per Gemini docs, `mimeType` is REQUIRED on `fileData` —
    /// the optional `ContentPart::ImageUrl.mime` is required
    /// for the Vertex path; mime-less URL-source content
    /// silently drops.
    FileData {
        #[serde(rename = "fileData")]
        file_data: VxFileData,
    },
}

#[derive(Serialize, Debug)]
pub struct VxFileData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "fileUri")]
    pub file_uri: String,
}
```

`fileData` (camelCase) matches Gemini's REST convention — same
naming style as the existing `inlineData` / `functionCall` /
`functionResponse` variants.

### A.2 — `message_to_vx` mapping

The existing Phase-22.A silent-drop arm at line 240-247 becomes
a real mapping:

```rust
ContentPart::ImageUrl { url, mime } => {
    // Phase 23 — Vertex's `fileData` requires a `mimeType`. If
    // the optional `mime` from the core type is None, silently
    // drop (matches the empty-text drop policy elsewhere).
    let Some(mime) = mime else {
        continue;
    };
    if !is_supported_vertex_mime(mime) {
        continue;
    }
    parts.push(VxPart::FileData {
        file_data: VxFileData {
            mime_type: mime.clone(),
            file_uri: url.clone(),
        },
    });
}
```

URL-scheme branching: tako does not pre-validate `gs://` vs.
`https://` vs. Vertex File API URIs — Gemini's API rejects
unsupported schemes at request time, which is the right error
surface (matches their own validation; same pattern as Phase
22.B's choice on Anthropic's `https`-only constraint).

### A.3 — Tests

Five new unit tests in
[`crates/tako-providers/vertex/src/convert.rs`](crates/tako-providers/vertex/src/convert.rs):

1. `image_url_block_emits_file_data_with_gs_uri` — `url:
   "gs://bucket/cat.jpg", mime: Some("image/jpeg")` → pinned
   JSON shape with `fileData.fileUri = "gs://bucket/cat.jpg"`,
   `fileData.mimeType = "image/jpeg"`.
2. `image_url_block_emits_file_data_with_https_uri` — `url:
   "https://example.com/cat.jpg", mime: Some("image/jpeg")` →
   `fileUri = "https://..."`. Confirms HTTPS URL pass-through
   identical to GCS.
3. `image_url_block_drops_when_mime_missing` — `mime: None` →
   no `fileData` part emitted.
4. `image_url_block_drops_unsupported_mime` — `mime:
   Some("image/svg+xml")` → silent-drop, matching the
   `is_supported_vertex_mime` filter that 20.A already applies
   to inline-data parts.
5. `image_url_and_inline_data_can_coexist` — mixed
   `ContentPart::Image` (inline base64) and
   `ContentPart::ImageUrl` (URL) parts in a single message
   serialise to two adjacent `parts` entries
   (`inlineData` + `fileData`) in source order.

## B. Cleanup

The Phase-22.A silent-drop comment on Vertex (`// Phase 22.A —
silent-drop. Vertex's fileData ...`) goes away with the real
implementation. Bedrock and Ollama keep their stubs — both
need a real SSRF design story before they can wire URL-source.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  `image_url_*` tests in 23.A.3 pass; existing Phase 20.A
  inline-data tests still byte-for-byte green (regression: the
  `InlineData` path must NOT change wire shape).

No new Python tests required — Phase 22.D's
[`tests/python/test_phase22_image_url.py`](tests/python/test_phase22_image_url.py)
already covers the Pydantic `ContentPart` surface that all
URL-source-aware adapters consume.

## Out of scope (Phase 24+)

- **Vertex File API upload flow** — needs a separate API
  surface for uploading bytes and getting back a Vertex File API
  URI; not just a content-block mapping.
- **Bedrock URL source** — AWS SDK's `ImageSource` has no URL
  variant; would need tako-side pre-fetch with SSRF guard.
- **Ollama URL source** — `images` field carries bare base64;
  same pre-fetch concern.
- **`http://` (non-TLS) URL hardening** — Phase 22 + 23 both
  pass URLs through verbatim; vendors reject non-HTTPS URLs at
  their own API boundary.
- mTLS / refresh-token / eval-graders / ChainedAuth
  short-circuit semantics carried over from Phase 21.

## Commits

1. `feat(tako-providers/vertex): URL-source images via VxPart::FileData (Phase 23.A)`
2. `docs: Phase 23 PLAN/README/CHANGELOG flip (v0.24.0)`
