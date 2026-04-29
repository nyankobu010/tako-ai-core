# Recipe: Vertex AI (Gemini)

The Vertex provider talks to Google's `:generateContent` REST API. Auth
is *deferred* — you supply a pre-resolved OAuth2 access token, and the
provider doesn't refresh it. This keeps the dependency tree small and
lets you wire whatever credential source you have.

## Quick start

```bash
export VERTEX_PROJECT_ID="my-gcp-project"
export VERTEX_ACCESS_TOKEN="$(gcloud auth print-access-token)"
```

```python
import os
import tako

provider = tako.providers.Vertex(
    project_id=os.environ["VERTEX_PROJECT_ID"],
    model="gemini-2.0-pro",
    access_token=os.environ["VERTEX_ACCESS_TOKEN"],
    location="us-central1",
)

agent = tako.SingleAgent(provider=provider, max_steps=4)
result = await agent.run("In one sentence: what is an octopus?")
```

## Refreshing tokens

`gcloud auth print-access-token` returns a token that's valid for ~1
hour. For long-lived processes, refresh on a schedule:

```python
import asyncio
import subprocess

def fresh_token() -> str:
    return subprocess.check_output(
        ["gcloud", "auth", "print-access-token"], text=True,
    ).strip()

async def with_refreshing_provider():
    while True:
        provider = tako.providers.Vertex(
            project_id="my-proj",
            model="gemini-2.0-pro",
            access_token=fresh_token(),
        )
        agent = tako.SingleAgent(provider=provider)
        # Use agent for ~50 minutes...
        await asyncio.sleep(50 * 60)
```

For GKE / Cloud Run, prefer the metadata server flow over `gcloud`.

## Tool calling

Gemini's tool-calling shape (`functionCall` / `functionResponse`) is
mapped automatically:

```python
agent = tako.SingleAgent(
    provider=tako.providers.Vertex(
        project_id="my-proj",
        model="gemini-2.0-pro",
        access_token=token,
    ),
    mcp_servers=[
        tako.mcp.Stdio(command="npx", args=["-y", "@modelcontextprotocol/server-everything"]),
    ],
)
result = await agent.run("Search for the weather in Tokyo and summarize.")
```

When the orchestrator emits a `ToolResult` for a previous tool call,
the converter looks up the call's id in the conversation history to
recover the function name (Vertex's `functionResponse` schema requires
it; tako's `ContentPart::ToolResult` only carries the call id).
