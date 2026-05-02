"""OTLP exporter wiring smoke test.

Doesn't require a running collector — verifies that:

1. ``init_otlp`` succeeds with a valid endpoint string (the gRPC connection
   is lazy; the BatchSpanProcessor doesn't block on the first export).
2. A subsequent orchestrator call doesn't error even though the export will
   silently fail in the background.
3. Re-initialisation in the same process is correctly rejected — the
   underlying ``tracing-subscriber::registry().try_init()`` is a one-shot
   per process by design.

A full end-to-end test against an in-process gRPC collector lives in
``crates/tako-governance/tests/otlp_collector_e2e.rs`` (Phase 47): a
``tonic`` mock implementing ``TraceService::Export`` receives spans
emitted via ``init_otlp_tracing`` and asserts on names + resource +
span attributes. This Python file exercises the **lifecycle / facade
contract** (init → run → re-init rejected → shutdown idempotent);
span content lives on the Rust side where the wire path runs.

NOTE: ``tracing-subscriber`` is process-wide and cannot be replaced.
Once ``init_otlp`` succeeds in a process, calling it again raises
``ValueError`` regardless of whether ``shutdown_otlp`` was called in
between. This is a tracing-rs constraint, not a tako one.
"""

from __future__ import annotations

import pytest
import tako


async def test_otlp_lifecycle_and_orchestrator_run() -> None:
    # Pick a port that's almost certainly closed; the gRPC connection is
    # lazy so init still returns Ok. Spans get queued and dropped silently.
    tako.tracing.init_otlp("http://127.0.0.1:14317")

    # Orchestrator run while OTLP is attached must succeed even though the
    # collector is unreachable.
    fake = tako.providers.Fake(canned_text="ok")
    agent = tako.SingleAgent(provider=fake)
    result = await agent.run("hi")
    assert result.text == "ok"

    # Re-init fails — explicit guard or subscriber-already-set, either way
    # a ValueError makes it through to Python.
    with pytest.raises(ValueError):
        tako.tracing.init_otlp("http://127.0.0.1:14318")

    # Shutdown is idempotent.
    tako.tracing.shutdown_otlp()
    tako.tracing.shutdown_otlp()
