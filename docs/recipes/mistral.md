# Mistral

The Mistral provider speaks the OpenAI-compatible REST surface that
mistral.ai exposes (and that any compatible self-hosted gateway speaks).

## Construct

```python
import os
import tako

provider = tako.providers.Mistral(
    model="mistral-large-latest",
    api_key=os.environ["MISTRAL_API_KEY"],
)

agent = tako.SingleAgent(provider=provider, max_steps=4)
result = await agent.run("Summarise: octopuses use tools.")
print(result.text)
```

## Streaming

```python
async for ev in agent.stream("Explain CRDTs"):
    if isinstance(ev, tako.OrchEvent.AssistantText):
        print(ev.text, end="", flush=True)
```

## Vision

Mistral's vision API is OpenAI-compatible. Inline base64 and URL-source
images both work — the vendor's API server fetches the URL.

```python
from tako import ChatRequest, ContentPart, Message, Role

req = ChatRequest(messages=[
    Message(role=Role.User, content=[
        ContentPart.text("What's in this image?"),
        ContentPart.image_url(url="https://example.com/photo.jpg"),
    ]),
])
```

## Self-hosted gateway

If you point a private gateway at the OpenAI-compatible surface, pass
`base_url=` to override the upstream:

```python
provider = tako.providers.Mistral(
    model="mistral-medium",
    api_key=os.environ["GATEWAY_KEY"],
    base_url="https://llm.internal.corp/v1",
)
```

## See also

- [Concepts → Providers](../concepts/providers.md)
- [Concepts → Vision content](../concepts/vision.md)
