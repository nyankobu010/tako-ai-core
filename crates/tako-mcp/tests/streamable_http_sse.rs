//! Phase 12.A — integration tests for `StreamableHttpTransport`'s SSE
//! notifications channel against a `wiremock` server. The mock returns
//! a fully-buffered `text/event-stream` body; the client opens a long-
//! lived `GET` on first `notifications()`, parses each `data:` frame as
//! JSON-RPC, and broadcasts method-bearing frames (no `id`) to every
//! subscriber. Frames carrying an `id` are dropped — those are POST
//! responses delivered inline by `request()`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use futures::stream::StreamExt;
use tako_core::McpTransport;
use tako_mcp::StreamableHttpTransport;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TWO_NOTIFS: &str = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{\"n\":1}}\n\n\
                          data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"n\":2}}\n\n";

#[tokio::test]
async fn sse_notifications_fan_out_in_order() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TWO_NOTIFS),
        )
        .mount(&server)
        .await;

    let t = StreamableHttpTransport::builder()
        .url(server.uri())
        .build()
        .unwrap();
    let mut stream = t.notifications().await;
    let collected = tokio::time::timeout(Duration::from_secs(2), async {
        let mut out = Vec::new();
        for _ in 0..2 {
            if let Some(item) = stream.next().await {
                out.push(item.unwrap());
            }
        }
        out
    })
    .await
    .unwrap();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0]["method"], "notifications/initialized");
    assert_eq!(collected[0]["params"]["n"], 1);
    assert_eq!(collected[1]["method"], "notifications/progress");
    assert_eq!(collected[1]["params"]["n"], 2);
    t.close().await.unwrap();
}

#[tokio::test]
async fn sse_multiple_subscribers_share_upstream() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TWO_NOTIFS),
        )
        .expect(1) // The transport must open exactly one upstream GET.
        .mount(&server)
        .await;

    let t = StreamableHttpTransport::builder()
        .url(server.uri())
        .build()
        .unwrap();
    let mut sub_a = t.notifications().await;
    let mut sub_b = t.notifications().await;
    let collected = tokio::time::timeout(Duration::from_secs(2), async {
        let mut a = Vec::new();
        let mut b = Vec::new();
        for _ in 0..2 {
            if let Some(item) = sub_a.next().await {
                a.push(item.unwrap());
            }
            if let Some(item) = sub_b.next().await {
                b.push(item.unwrap());
            }
        }
        (a, b)
    })
    .await
    .unwrap();
    assert_eq!(collected.0.len(), 2);
    assert_eq!(collected.1.len(), 2);
    assert_eq!(collected.0[0]["method"], "notifications/initialized");
    assert_eq!(collected.1[0]["method"], "notifications/initialized");
    t.close().await.unwrap();
    // wiremock verifies `expect(1)` on Drop.
}

#[tokio::test]
async fn sse_response_frames_with_id_are_not_broadcast() {
    // Frames carrying `id` are POST responses; the SSE channel is
    // server→client notifications only. Verify the transport drops
    // id-bearing frames silently rather than yielding them as
    // notifications.
    let server = MockServer::start().await;
    let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ignored\":true}}\n\n\
                data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/log\",\"params\":{\"msg\":\"hi\"}}\n\n";
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let t = StreamableHttpTransport::builder()
        .url(server.uri())
        .build()
        .unwrap();
    let mut stream = t.notifications().await;
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // The id-bearing frame must have been skipped; we receive the
    // method-bearing frame as the first notification.
    assert_eq!(first["method"], "notifications/log");
    assert_eq!(first["params"]["msg"], "hi");
    t.close().await.unwrap();
}

#[tokio::test]
async fn sse_get_attaches_mcp_session_id_after_post_handshake() {
    // The transport captures `Mcp-Session-Id` from a POST response and
    // must attach it to subsequent GETs. We POST first to seed the
    // session, then assert the GET carried the header.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .insert_header("Mcp-Session-Id", "sess-abc")
                .set_body_string(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header_exists("Mcp-Session-Id"))
        .and(header("Mcp-Session-Id", "sess-abc"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/ready\",\"params\":{}}\n\n",
                ),
        )
        .expect(1)
        .mount(&server)
        .await;

    let t = StreamableHttpTransport::builder()
        .url(server.uri())
        .build()
        .unwrap();
    // Seed the session id via a POST.
    let _ = t.request("ping", serde_json::json!({})).await.unwrap();
    let mut stream = t.notifications().await;
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first["method"], "notifications/ready");
    t.close().await.unwrap();
    // wiremock verifies `expect(1)` (matched by header) on Drop.
}
