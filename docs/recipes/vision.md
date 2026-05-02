# Vision content

Pass an image alongside a prompt. Works on all seven SDK-backed
providers.

## Inline image (bytes you already have)

```python
import base64
import tako
from tako import ChatRequest, ContentPart, Message, Role

with open("photo.jpg", "rb") as f:
    jpeg_b64 = base64.b64encode(f.read()).decode()

req = ChatRequest(messages=[
    Message(role=Role.User, content=[
        ContentPart.text("Describe this image."),
        ContentPart.image(mime="image/jpeg", data_b64=jpeg_b64),
    ]),
])

agent = tako.SingleAgent(provider=tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."))
result = await agent.run(req)
print(result.text)
```

## URL-source image (vendor fetches)

For Anthropic, OpenAI, Mistral, and Vertex (Gemini), the vendor's API
server fetches the URL — there is no tako-side network egress.

```python
req = ChatRequest(messages=[
    Message(role=Role.User, content=[
        ContentPart.text("What's the colour of the dome?"),
        ContentPart.image_url(url="https://example.com/photo.jpg"),
    ]),
])
```

Vertex's `fileData` part also accepts `gs://` GCS URIs; Google fetches
those server-side too.

## URL-source image on Bedrock or Ollama

Bedrock and Ollama don't accept URLs in their wire formats. To use
`ContentPart::ImageUrl` on those providers, *tako* fetches the URL
locally and rewrites it to inline before the call. This is opt-in and
default-off because of the SSRF surface:

```python
provider = tako.providers.Bedrock(
    region="us-east-1",
    model_id="anthropic.claude-3-5-sonnet-20241022-v2:0",
    url_prefetch=True,
)
```

See [URL pre-fetch & SSRF](../concepts/url_prefetch.md) for the
complete mitigation stack (private-IP blocklist, DNS-rebind defence,
allowlist override).

## Supported MIME types

`image/jpeg`, `image/png`, `image/gif`, `image/webp` on every provider.
Other types are silently dropped at adapter time (no exception
raised) — match the empty-text drop policy elsewhere.

## See also

- [Concepts → Vision content](../concepts/vision.md) — per-provider wire shapes.
- [Concepts → URL pre-fetch & SSRF](../concepts/url_prefetch.md)
