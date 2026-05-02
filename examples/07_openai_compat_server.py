"""Phase 2 example: serve any tako orchestrator behind an OpenAI-compatible
HTTP API and call it with the official `openai` Python SDK.

Run:
    pip install tako openai
    python examples/07_openai_compat_server.py
"""

from __future__ import annotations

import os

import tako


def main() -> None:
    fake = tako.providers.Fake(canned_text="hello from the compat server")
    agent = tako.SingleAgent(provider=fake)

    url = tako.compat.serve_openai(
        agent,
        host="127.0.0.1",
        port=8080,
        tokens={"sk-tako-demo": ("acme", "alice")},
        models=["tako-demo"],
    )
    print(f"serving at {url}")
    print("Try:")
    print(
        f"  curl -s {url}/v1/chat/completions "
        f'-H "Authorization: Bearer sk-tako-demo" '
        f'-H "Content-Type: application/json" '
        f'-d \'{{"model":"tako-demo","messages":[{{"role":"user","content":"hi"}}]}}\''
    )

    # Example with the openai SDK (if installed):
    if os.getenv("DEMO_USE_OPENAI"):
        from openai import OpenAI

        client = OpenAI(api_key="sk-tako-demo", base_url=f"{url}/v1")
        resp = client.chat.completions.create(
            model="tako-demo",
            messages=[{"role": "user", "content": "hi"}],
        )
        print(resp.choices[0].message.content)
    else:
        print("(set DEMO_USE_OPENAI=1 to also exercise the openai SDK)")

    input("press Enter to shut down... ")
    tako.compat.shutdown_openai()


if __name__ == "__main__":
    main()
