# Vision content

Every SDK-backed provider in `tako` accepts vision content as part of an
ordinary `ChatRequest`. There are two shapes — inline base64 and URL
source — selected by `ContentPart` variant.

## Two shapes

```python
from tako import ChatRequest, ContentPart, Message, Role

# Inline: bytes you already have in memory or read from disk.
inline = ContentPart.image(mime="image/jpeg", data_b64=jpeg_b64)

# URL: vendor's API server fetches the URL, OR (Bedrock + Ollama only)
# tako pre-fetches it locally.
remote = ContentPart.image_url(url="https://example.com/photo.jpg")

req = ChatRequest(messages=[
    Message(role=Role.User, content=[
        ContentPart.text("Describe this image."),
        inline,
        remote,
    ]),
])
```

Both shapes interoperate — a single message may carry any mix of text,
inline images, and URL-source images.

## Per-provider wire shapes

The `tako` adapter for each provider translates `ContentPart::Image` /
`ContentPart::ImageUrl` into the right vendor wire shape:

| Provider | Inline shape | URL shape |
|----------|--------------|-----------|
| Anthropic | `{"type": "image", "source": {"type": "base64", ...}}` | `{"type": "image", "source": {"type": "url", "url": "..."}}` |
| OpenAI | `{"type": "image_url", "image_url": {"url": "data:..."}}` | `{"type": "image_url", "image_url": {"url": "https://..."}}` |
| Mistral | OpenAI-compatible | OpenAI-compatible |
| Vertex (Gemini) | `inlineData: {mimeType, data}` | `fileData: {mimeType, fileUri}` (accepts `gs://` GCS or `https://`) |
| Bedrock | `image.source.bytes` | tako pre-fetches → inline |
| Ollama | sibling `images: [bare-base64]` field on the message | tako pre-fetches → inline |

For the three providers whose API server fetches URLs (Anthropic +
OpenAI + Mistral) and Vertex with its `fileData` part, the security
posture is identical to a direct vendor call — no tako-side network
egress. For Bedrock + Ollama, see [URL pre-fetch & SSRF](url_prefetch.md)
for the SSRF mitigation stack `tako` applies.

## Supported MIME types

All providers accept `image/jpeg`, `image/png`, `image/gif`, and
`image/webp`. Other types are silently dropped at adapter time
(matching the empty-text drop policy elsewhere — adapters never raise on
content shape, they just don't emit unsupported parts).

## Data-URL prefix normalisation

Inline content accepts either bare base64 or a `data:image/...;base64,...`
prefix interchangeably. Adapters strip the prefix before serialisation.

## Vendor URL fetch vs. tako pre-fetch

| Path | Used by | Security notes |
|------|---------|----------------|
| Vendor server-side fetch | Anthropic, OpenAI, Mistral, Vertex | Zero egress from tako. Equivalent to a direct vendor call. |
| tako-side pre-fetch | Bedrock, Ollama | Off by default. When on, full SSRF mitigation stack applies — see [URL pre-fetch & SSRF](url_prefetch.md). |

## See also

- [URL pre-fetch & SSRF](url_prefetch.md) — Bedrock + Ollama path.
- [Providers](providers.md) — capability table.
- [recipes/vision.md](../recipes/vision.md) — copy-pasteable example.
