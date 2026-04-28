"""Phase 1.5 preview: custom Python provider via tako.providers.Fake.

Today, defining a fully custom Rust-backed provider in pure Python
requires the PyPythonProvider FFI shim, which arrives in Phase 1.5. As a
stand-in, the Fake provider lets you bench the orchestrator loop with
canned responses.
"""

from __future__ import annotations

import asyncio

import tako


async def main() -> None:
    fake = tako.providers.Fake(canned_text="hello from a fake provider", id="example:fake")
    agent = tako.SingleAgent(provider=fake)
    result = await agent.run("anything")
    print(result.text)
    print(f"calls: {fake.call_count}")


if __name__ == "__main__":
    asyncio.run(main())
