"""Phase 2.5 example: SingleAgent backed by Vertex AI (Gemini).

The Vertex provider does not refresh OAuth2 tokens. For local dev, the
quickest path is::

    export VERTEX_PROJECT_ID="my-gcp-project"
    export VERTEX_ACCESS_TOKEN="$(gcloud auth print-access-token)"
    python examples/10_vertex_gemini.py

For long-lived processes, wire your own credential source (gcp_auth, the
GKE metadata server, or a service-account JWT exchange) and rebuild the
provider before tokens expire.
"""

from __future__ import annotations

import asyncio
import os

import tako


async def main() -> None:
    project_id = os.getenv("VERTEX_PROJECT_ID")
    access_token = os.getenv("VERTEX_ACCESS_TOKEN")
    location = os.getenv("VERTEX_LOCATION", "us-central1")
    model = os.getenv("VERTEX_MODEL", "gemini-2.0-pro")

    if not (project_id and access_token):
        provider: tako.providers._ProviderBase = tako.providers.Fake(
            canned_text="(skipped: VERTEX_PROJECT_ID / VERTEX_ACCESS_TOKEN not set)",
        )
    else:
        provider = tako.providers.Vertex(
            project_id=project_id,
            model=model,
            access_token=access_token,
            location=location,
        )

    agent = tako.SingleAgent(provider=provider, max_steps=4)
    result = await agent.run(
        "In one sentence: what is an octopus?",
        tenant_id="demo",
        user_id="alice",
    )
    print(f"[{provider.id}] {result.text}")


if __name__ == "__main__":
    asyncio.run(main())
