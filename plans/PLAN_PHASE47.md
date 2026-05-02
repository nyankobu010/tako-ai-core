# PLAN — Phase 47 (OTel real-collector e2e test)

> **Status: in progress.** Targets v0.48.0. Closes the
> "OTel end-to-end test against a real gRPC collector"
> carry-forward item from
> [PLAN.md](../PLAN.md) (originally deferred from
> Phase 1.5 acceptance criteria).

## Context

`tako-governance::init_otlp_tracing` builds a real OTLP gRPC
exporter via `tonic` and attaches it to `tracing-subscriber`.
Today's test coverage:

| Test | What it proves |
|------|----------------|
| Lib unit tests in `crates/tako-governance/src/otel.rs` | `TracingConfig` (de)serialisation. |
| [`tests/python/test_otlp.py`](../tests/python/test_otlp.py) | `init_otlp` doesn't error against an unreachable endpoint; orchestrator runs while OTLP is attached; re-init is rejected. |

Neither test asserts that **spans actually arrive** at a collector. The
orchestrator emits `tako.orchestrator.run` and `tako.provider.chat`
spans (see [`crates/tako-orchestrator/src/single.rs:230-266`](../crates/tako-orchestrator/src/single.rs#L230-L266)),
but no test verifies they make it through the OTLP exporter,
across the gRPC wire, into a collector, with the right names
and `gen_ai.*` semconv attributes.

The PLAN.md backlog flags this:

> **OTel end-to-end test against a real gRPC collector.** Full
> e2e test deferred from Phase 1.5 acceptance criteria.

Phase 47 closes the gap with a self-contained `tonic`-based
mock collector that:

1. Implements the `opentelemetry-proto` `TraceService::Export`
   server trait.
2. Listens on a random local port.
3. Buffers received `ResourceSpans` so tests can assert.

No external dependency (no `otelcol-contrib` binary, no
Docker). Same self-contained pattern as Phase 42's
`oidc_mtls_e2e.rs` (per-test `axum-server` + `rustls`).

## Why now

After Phase 46 the open backlog has two items:

1. **OTel real-collector e2e test** (this phase) — clear
   scope, ~300 lines test infra, mirrors the Phase 42 wire-
   level test pattern.
2. **Eval harness real graders** — large; needs sandboxed-
   container design.

Item 1 is the higher-value, lower-risk pick. It closes a
long-standing Phase 1.5 carry-forward and provides real
defensive value: today a regression in span emission (e.g.
breaking change in `tracing-opentelemetry` causing spans to
silently drop) wouldn't be caught by any test.

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 47.A | Add `opentelemetry-proto` (`gen-tonic` + `trace`) as a `tako-governance` **dev-dep**. No runtime dep change. | [`crates/tako-governance/Cargo.toml`](../crates/tako-governance/Cargo.toml) |
| 47.B | New integration test `crates/tako-governance/tests/otlp_collector_e2e.rs` with an in-process `tonic` mock collector. | new file |
| 47.C | Update `tests/python/test_otlp.py` docstring to remove the "Phase 2 deferred" caveat | [`tests/python/test_otlp.py`](../tests/python/test_otlp.py) |
| 47.D | Workspace + Python version 0.47.0 → 0.48.0 | various |
| 47.E | PLAN.md row + close backlog item | [`PLAN.md`](../PLAN.md) |
| 47.F | CHANGELOG.md `[0.48.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |

## What this phase will land

### 47.A — Dev-dep addition

```toml
[dev-dependencies]
# ... existing entries kept ...
opentelemetry-proto = { version = "0.31", default-features = false, features = ["gen-tonic", "gen-tonic-messages", "trace"] }
tonic-prost         = { workspace = true }  # gen-tonic re-exports under this
```

`opentelemetry-proto` 0.31 with `gen-tonic` exposes the
`opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{TraceService, TraceServiceServer}`
trait + service that we implement to act as a mock
collector. Already a transitive dep via `opentelemetry-otlp`,
so this just widens visibility — no new download.

### 47.B — Mock collector + e2e test

`crates/tako-governance/tests/otlp_collector_e2e.rs` (gated
to `cfg(test)`):

```rust
//! Phase 47 — OTel real-collector e2e test.
//!
//! Spawns a `tonic` mock that implements `TraceService::Export`,
//! buffering received spans. Initialises tracing with
//! `init_otlp_tracing` pointed at the mock, emits a span,
//! and asserts on what the mock received.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tonic::{Request, Response, Status};
use opentelemetry_proto::tonic::collector::trace::v1::{
    trace_service_server::{TraceService, TraceServiceServer},
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::trace::v1::ResourceSpans;
use tracing::info_span;
use tako_governance::{init_otlp_tracing, TracingConfig};

#[derive(Default, Clone)]
struct MockCollector {
    inbox: Arc<Mutex<Vec<ResourceSpans>>>,
}

#[tonic::async_trait]
impl TraceService for MockCollector {
    async fn export(
        &self,
        req: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        let payload = req.into_inner();
        let mut buf = self.inbox.lock().unwrap();
        buf.extend(payload.resource_spans);
        Ok(Response::new(ExportTraceServiceResponse::default()))
    }
}

async fn spawn_mock_collector() -> (MockCollector, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = format!("http://{}", addr);
    let collector = MockCollector::default();
    let svc = TraceServiceServer::new(collector.clone());
    tokio::spawn(async move {
        let stream = tokio_stream::wrappers::TcpListenerStream::new(listener);
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(stream)
            .await
            .ok();
    });
    // Give the server a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (collector, endpoint)
}

#[tokio::test(flavor = "multi_thread")]
async fn spans_arrive_at_mock_collector() {
    let (mock, endpoint) = spawn_mock_collector().await;

    let guard = init_otlp_tracing(&TracingConfig {
        otlp_endpoint: Some(endpoint),
        ..Default::default()
    })
    .expect("init_otlp_tracing");

    {
        let _enter = info_span!("test_span", marker = "phase47").entered();
    }

    // BatchSpanProcessor flushes on shutdown — drop the guard to flush.
    guard.shutdown();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let received = mock.inbox.lock().unwrap();
    assert!(!received.is_empty(), "no ResourceSpans arrived");

    let names: Vec<String> = received
        .iter()
        .flat_map(|rs| rs.scope_spans.iter())
        .flat_map(|ss| ss.spans.iter())
        .map(|s| s.name.clone())
        .collect();
    assert!(
        names.iter().any(|n| n == "test_span"),
        "expected `test_span` in {names:?}"
    );
}
```

Plus a second test that exercises `tako.orchestrator.run` /
`tako.provider.chat` span emission via a `FakeProvider`-backed
`SingleAgent`, asserting:
- The `tako.orchestrator.run` span name appears.
- The `tako.orchestrator.kind` attribute = `"single"`.
- At least one `tako.provider.chat` child span.

This is the highest-value test — it proves the **full path**
(orchestrator → tracing → OTel → tonic → wire → collector)
works end-to-end.

**`tracing-subscriber` is process-wide** so multiple OTLP-init
tests in one binary collide. Two strategies:
- Run both tests in the same `#[tokio::test]` (single init).
- Or factor into a single test that emits both kinds of span.

Going with the single-test approach for simplicity — one
init, both assertions in sequence.

### 47.C — Update Python test docstring

Remove the now-misleading caveat:

```diff
-A full end-to-end test against an in-process gRPC collector is deferred
-to Phase 2 — it requires the ``opentelemetry-collector-contrib`` binary
-or a tonic-based mock, neither of which is in the Phase-1.5 acceptance
-criteria.
+A full end-to-end test against an in-process gRPC collector lands in
+Phase 47 (Rust integration test
+``crates/tako-governance/tests/otlp_collector_e2e.rs``). This Python
+test exercises the lifecycle / facade contract (init → run → re-init
+rejected → shutdown idempotent); span content is asserted on the
+Rust side where the wire path runs.
```

### 47.D — Version bump

0.47.0 → 0.48.0 across `Cargo.toml` (workspace + 14 internal
crate version pins), `pyproject.toml`, `python/tako/__init__.py`,
`tests/python/test_smoke.py`.

### 47.E — PLAN.md update

- New row `47 — OTel real-collector e2e test`.
- Flip "OTel end-to-end test against a real gRPC collector"
  backlog item to closed-by-Phase-47.

### 47.F — CHANGELOG `[0.48.0]`

Standard format, brief.

## Critical files

**Modified:**
- [`crates/tako-governance/Cargo.toml`](../crates/tako-governance/Cargo.toml) (47.A).
- [`tests/python/test_otlp.py`](../tests/python/test_otlp.py) — docstring (47.C).
- Standard PLAN/CHANGELOG/version flip.

**Created:**
- [`crates/tako-governance/tests/otlp_collector_e2e.rs`](../crates/tako-governance/tests/otlp_collector_e2e.rs) (47.B).
- [`plans/PLAN_PHASE47.md`](PLAN_PHASE47.md) (this file).

## Verification

1. `cargo fmt --all -- --check`.
2. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings`.
3. `cargo test -p tako-governance --test otlp_collector_e2e` — new e2e test passes.
4. `cargo test --workspace --exclude tako-py --all-features` — no regressions.
5. `ruff format --check` + `ruff check`.
6. `maturin develop --release` — wheel builds at v0.48.0.
7. `pytest -q` — full suite green; smoke pins v0.48.0.

## Out of scope

- **TLS / mTLS to the collector.** OTLP supports both; the
  exporter side is configured via `otlp_endpoint` URL scheme
  + standard env vars (`OTEL_EXPORTER_OTLP_CERTIFICATE`).
  The mock collector for this phase serves plain gRPC over
  `127.0.0.1:0` — sufficient for the wire-path proof.
  Production deployments use real collectors; we don't need
  to test the TLS path against a mock.
- **HTTP/protobuf OTLP.** `tako-governance` only exposes the
  gRPC path today (`with_tonic()` in `init_otlp_tracing`).
  HTTP/protobuf is a separate roadmap item.
- **Metrics + logs OTLP.** `tako-governance` only exports
  traces. Metrics + logs would each need their own
  init/exporter; out of scope.
- **Asserting on ALL `gen_ai.*` semconv attributes.** The
  `tracing-opentelemetry` layer maps `tracing` field
  attributes via standard semconv translation. Asserting on
  every attribute name + value would tightly couple the
  test to the layer implementation. We assert on the
  invariants that matter (span names + the `tako.*`
  attributes we emit explicitly) and trust the upstream
  layer's semconv translation.
- **Re-init scenarios across multiple tests.** The
  process-wide `tracing-subscriber` constraint means we run
  one OTLP init per test binary. Acceptable.
