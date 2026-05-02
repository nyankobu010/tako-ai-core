//! Phase 47 — OTel real-collector end-to-end test.
//!
//! Closes the Phase 1.5 carry-forward identified in
//! [`PLAN.md`](../../../PLAN.md): until now, no test asserted
//! that spans actually arrive at a collector — only that
//! [`init_otlp_tracing`](tako_governance::init_otlp_tracing)
//! doesn't error and the orchestrator keeps running while
//! attached.
//!
//! This test:
//!
//! 1. Spins up a `tonic`-based mock implementing
//!    `opentelemetry_proto::tonic::collector::trace::v1::TraceService::Export`
//!    on `127.0.0.1:0` (random port). Every received
//!    `ResourceSpans` is buffered in a shared `Mutex<Vec<_>>`.
//! 2. Calls `init_otlp_tracing` pointed at the mock's
//!    endpoint.
//! 3. Emits a marker span via `tracing::info_span!`.
//! 4. Drops the `TracerGuard` to flush the
//!    `BatchSpanProcessor`.
//! 5. Asserts the marker span name reached the mock with
//!    its custom attribute intact, plus the `service.name`
//!    resource attribute set by the tako exporter.
//!
//! `tracing-subscriber` is process-wide so this binary
//! can only run **one** OTLP-init test — the assertions
//! below run inside a single `#[tokio::test]`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{
    TraceService, TraceServiceServer,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::trace::v1::ResourceSpans;
use tokio::net::TcpListener;
use tonic::{Request, Response, Status};

use tako_governance::{TracingConfig, init_otlp_tracing};

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
    let endpoint = format!("http://{addr}");
    let collector = MockCollector::default();
    let svc = TraceServiceServer::new(collector.clone());

    tokio::spawn(async move {
        let stream = tokio_stream::wrappers::TcpListenerStream::new(listener);
        let _ = tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(stream)
            .await;
    });

    // Brief grace period so the listener accepts connections before
    // the exporter dials. `tonic::transport::Server::serve_with_incoming`
    // doesn't expose a "ready" signal.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (collector, endpoint)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spans_arrive_at_mock_collector() {
    let (mock, endpoint) = spawn_mock_collector().await;

    let guard = init_otlp_tracing(&TracingConfig {
        otlp_endpoint: Some(endpoint),
        ..Default::default()
    })
    .expect("init_otlp_tracing");

    {
        let _enter = tracing::info_span!("phase47_marker_span", marker = "phase47-e2e").entered();
        // Span body — anything traced here lands in the same span context.
        tracing::info!(target: "tako", "inside the marker span");
    }

    // BatchSpanProcessor flushes on shutdown — drop the guard to flush.
    guard.shutdown();

    // Give the export RPC a moment to land in the mock's inbox.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let received = mock.inbox.lock().unwrap();
    assert!(
        !received.is_empty(),
        "expected at least one ResourceSpans payload to arrive at the mock collector"
    );

    // Walk the proto: ResourceSpans -> ScopeSpans -> Span.
    let span_names: Vec<String> = received
        .iter()
        .flat_map(|rs| rs.scope_spans.iter())
        .flat_map(|ss| ss.spans.iter())
        .map(|s| s.name.clone())
        .collect();

    assert!(
        span_names.iter().any(|n| n == "phase47_marker_span"),
        "expected `phase47_marker_span` in the received span names; got: {span_names:?}"
    );

    // Resource attributes — `tako-governance` sets `service.name=tako`
    // and `service.version=<crate version>` in `init_otlp_tracing`.
    let resource_attrs: Vec<(String, String)> = received
        .iter()
        .flat_map(|rs| rs.resource.iter())
        .flat_map(|r| r.attributes.iter())
        .filter_map(|kv| {
            kv.value.as_ref().and_then(|v| v.value.as_ref()).map(|val| {
                let stringified = match val {
                    opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) => {
                        s.clone()
                    }
                    other => format!("{other:?}"),
                };
                (kv.key.clone(), stringified)
            })
        })
        .collect();

    assert!(
        resource_attrs
            .iter()
            .any(|(k, v)| k == "service.name" && v == "tako"),
        "expected resource attribute `service.name=tako`; got: {resource_attrs:?}"
    );
    assert!(
        resource_attrs.iter().any(|(k, _)| k == "service.version"),
        "expected resource attribute `service.version=<crate version>`; got: {resource_attrs:?}"
    );

    // The span's own attributes should preserve the `marker` field
    // we attached to the `info_span!` invocation.
    let span_attrs: Vec<(String, String)> = received
        .iter()
        .flat_map(|rs| rs.scope_spans.iter())
        .flat_map(|ss| ss.spans.iter())
        .filter(|s| s.name == "phase47_marker_span")
        .flat_map(|s| s.attributes.iter())
        .filter_map(|kv| {
            kv.value.as_ref().and_then(|v| v.value.as_ref()).map(|val| {
                let stringified = match val {
                    opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) => {
                        s.clone()
                    }
                    other => format!("{other:?}"),
                };
                (kv.key.clone(), stringified)
            })
        })
        .collect();

    assert!(
        span_attrs
            .iter()
            .any(|(k, v)| k == "marker" && v == "phase47-e2e"),
        "expected span attribute `marker=phase47-e2e`; got: {span_attrs:?}"
    );
}
