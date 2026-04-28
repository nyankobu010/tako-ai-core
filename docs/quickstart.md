# Quickstart

## Install

```bash
pip install tako
```

No Rust toolchain is required at install time — wheels are prebuilt for
manylinux, musllinux, macOS universal2, and Windows x64/arm64.

## Hello world

```python
import asyncio
import os

import tako

async def main() -> None:
    provider = tako.providers.Anthropic(
        model="claude-opus-4-7",
        api_key=os.environ["ANTHROPIC_API_KEY"],
    )
    agent = tako.SingleAgent(provider=provider, max_steps=4)
    result = await agent.run("In one sentence: what is an octopus?")
    print(result.text)

asyncio.run(main())
```

## Synchronous API

Every async method has a `_sync` sibling:

```python
result = agent.run_sync("Quick question: ...")
```

The sync sibling releases the GIL while waiting for the response so other
Python threads can run.

## Without an API key

For local development and tests use the in-process Fake provider:

```python
provider = tako.providers.Fake(canned_text="hello")
agent = tako.SingleAgent(provider=provider)
result = agent.run_sync("anything")
assert result.text == "hello"
```

## Tracing

Process-wide tracing is one call away:

```python
import tako.tracing
tako.tracing.init(filter="tako=debug,info", json=True)
```

OTLP exporter integration arrives in Phase 2; the `tako.tracing.Otlp(...)`
type already exists so user code is forward-compatible.
