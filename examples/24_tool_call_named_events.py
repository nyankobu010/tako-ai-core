"""Phase 10.B: named tako.* SSE events for tool-call lifecycle.

The OpenAI-compat server already maps `OrchEvent::ToolCallStart` to
the OpenAI `tool_calls` delta. Phase 10.B adds two parallel named
SSE extension frames for tako-aware consumers:

    event: tako.tool_call_start
    data: {"step": 0, "name": "weather", "id": "tc-abc"}

    event: tako.tool_call_result
    data: {"step": 0, "id": "tc-abc",
           "result": {"temp_c": 18}, "is_error": false}

OpenAI clients ignore unknown ``event:`` lines per the SSE spec, so
adding the named frames is zero-impact for them; raw SSE consumers
that want a typed handle on the lifecycle subscribe by name.

This example boots a minimal compat server backed by a
``FakeProvider`` that emits no tool calls, so you can curl the
streaming endpoint and inspect the wire format directly. For a
realistic tool-call scenario, plug a tool-emitting provider into the
same harness — the same named ``tako.tool_call_*`` frames will
appear interleaved with the OpenAI ``data:`` chunks.
"""

from __future__ import annotations

import tako


def main() -> None:
    fake = tako.providers.Fake(canned_text="hello from compat server")
    agent = tako.SingleAgent(provider=fake)

    url = tako.compat.serve_openai(
        agent,
        host="127.0.0.1",
        port=0,  # let the OS pick a free port
        tokens={"sk-tako-demo": ("acme", "alice")},
        models=["tako-demo"],
    )
    print(f"serving at {url}")
    print()
    print("In another terminal, run:")
    print(f"  curl -N {url}/v1/chat/completions \\")
    print('       -H "Authorization: Bearer sk-tako-demo" \\')
    print('       -H "Content-Type: application/json" \\')
    print(
        '       -d \'{"model":"tako-demo","stream":true,'
        '"messages":[{"role":"user","content":"hi"}]}\''
    )
    print()
    print("Look for `event: tako.tool_call_start` / `tako.tool_call_result`")
    print("frames between the OpenAI `data: {chat.completion.chunk}` lines")
    print("when the orchestrator emits tool calls.")
    print()
    input("press Enter to shut down... ")
    tako.compat.shutdown_openai()


if __name__ == "__main__":
    main()
