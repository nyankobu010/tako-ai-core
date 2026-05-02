# Recipe: OpenAI-compatible HTTP server

`tako.compat.serve_openai` boots an `axum` server that wraps any
orchestrator behind the routes the official `openai` Python SDK speaks
— `/v1/chat/completions` (streaming + non-streaming), `/v1/models`,
`/healthz`, `/readyz`.

This is how you put `tako` in front of an existing OpenAI-SDK app
without changing client code: change the `base_url`, keep everything
else.

## Boot a server

```python
import tako

agent = tako.SingleAgent(
    provider=tako.providers.Anthropic(model="claude-opus-4-7", api_key="..."),
)

url = tako.compat.serve_openai(
    agent,
    host="127.0.0.1",
    port=8000,
    tokens={"sk-team-acme": ("acme", "alice")},  # token -> (tenant_id, user_id)
    models=["claude-opus-4-7", "tako-default"],
)
print(f"serving at {url}")
```

The `tokens` map is the bearer-token auth resolver: an incoming
`Authorization: Bearer sk-team-acme` is recognized and the resulting
`Principal { tenant_id: "acme", user_id: "alice" }` flows into the
orchestrator.

## Use it from the openai SDK

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:8000/v1", api_key="sk-team-acme")
resp = client.chat.completions.create(
    model="claude-opus-4-7",
    messages=[{"role": "user", "content": "hi"}],
)
print(resp.choices[0].message.content)
```

Streaming works too:

```python
stream = client.chat.completions.create(
    model="claude-opus-4-7",
    messages=[{"role": "user", "content": "hi"}],
    stream=True,
)
for chunk in stream:
    if chunk.choices[0].delta.content:
        print(chunk.choices[0].delta.content, end="", flush=True)
```

## Wiring per-tenant policy

The `Principal` resolved from the bearer token is available to any
`PolicyEngine` consulted by the orchestrator. To enforce
`max_usd_per_tenant_per_day` differently per tenant:

```python
budget = tako.Budget(
    max_usd_per_tenant_per_day={"acme": 50.0, "beta": 1000.0},
)
client_runtime = tako.Client(providers=[provider], budget=budget)
agent = tako.SingleAgent(provider=provider, client=client_runtime)
url = tako.compat.serve_openai(agent, ...)
```

## Shutdown

```python
tako.compat.shutdown_openai()
```

This gracefully drains in-flight requests; bind a signal handler if you
want SIGTERM-driven shutdown.
