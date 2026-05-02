# Ollama

The Ollama provider talks to a local (or networked) Ollama server. No
API key is required; the server itself is the auth boundary.

## Construct

```python
import tako

provider = tako.providers.Ollama(
    model="llama3.2",
    base_url="http://127.0.0.1:11434",
    timeout_secs=120,
)

agent = tako.SingleAgent(provider=provider, max_steps=4)
result = await agent.run("Why are octopuses good problem-solvers?")
print(result.text)
```

## Streaming

Ollama streams as NDJSON over `/api/chat`. The tako adapter exposes it
as ordinary `OrchEvent::AssistantText`:

```python
async for ev in agent.stream("Explain CRDTs"):
    if isinstance(ev, tako.OrchEvent.AssistantText):
        print(ev.text, end="", flush=True)
```

## Vision

Ollama uses a sibling `images: [bare-base64]` field on each message
rather than a content-block array. The tako adapter handles inline
images transparently — you still pass them as `ContentPart::Image`:

```python
from tako import ChatRequest, ContentPart, Message, Role

req = ChatRequest(messages=[
    Message(role=Role.User, content=[
        ContentPart.text("Describe this:"),
        ContentPart.image(mime="image/jpeg", data_b64=jpeg_b64),
    ]),
])
```

## URL-source images via tako pre-fetch

Ollama's wire format doesn't accept URLs, so for `ContentPart::ImageUrl`
content tako has to fetch the URL itself. This is opt-in and default-off
because of the SSRF surface — see [URL pre-fetch & SSRF](../concepts/url_prefetch.md)
for the full mitigation stack.

```python
provider = tako.providers.Ollama(
    base_url="http://127.0.0.1:11434",
    url_prefetch=True,
    url_prefetch_timeout_secs=10,
    url_prefetch_max_bytes=5 * 1024 * 1024,
    url_prefetch_allow_hosts=["*.internal.corp"],
    url_prefetch_allow_cidrs=["10.0.5.0/24"],
)
```

## See also

- [Concepts → URL pre-fetch & SSRF](../concepts/url_prefetch.md)
- [Concepts → Vision content](../concepts/vision.md)
