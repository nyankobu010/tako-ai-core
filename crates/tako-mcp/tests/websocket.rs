//! End-to-end WebSocketTransport tests against a tokio-tungstenite echo
//! server spawned in-process.
#![cfg(feature = "ws")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use futures::stream::StreamExt;
use futures::{SinkExt, future::join_all};
use tako_core::McpTransport;
use tako_mcp::WebSocketTransport;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

type RpcHandler = dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync + 'static;

/// Spin up a one-connection JSON-RPC test server that responds to any
/// `request` with the JSON returned by `handler` and emits the listed
/// `notifications` as soon as it accepts the connection. Returns the
/// `ws://...` URL.
async fn spawn_server(handler: Arc<RpcHandler>, notifications: Vec<serde_json::Value>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}");

    tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.unwrap();
        let ws = accept_async(stream).await.unwrap();
        let (sink, mut source) = ws.split();
        let sink = Arc::new(Mutex::new(sink));

        // Push notifications immediately on connect.
        for n in notifications {
            let payload = serde_json::to_string(&n).unwrap();
            sink.lock()
                .await
                .send(Message::Text(payload.into()))
                .await
                .unwrap();
        }

        while let Some(msg) = source.next().await {
            match msg {
                Ok(Message::Text(t)) => {
                    let req: serde_json::Value = serde_json::from_str(&t).unwrap();
                    let id = req.get("id").cloned();
                    if id.is_none() {
                        // Notification — no response.
                        continue;
                    }
                    let result = handler(req);
                    let resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result,
                    });
                    let payload = serde_json::to_string(&resp).unwrap();
                    sink.lock()
                        .await
                        .send(Message::Text(payload.into()))
                        .await
                        .unwrap();
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    url
}

async fn echo_handler(req: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "method": req.get("method").cloned(),
        "params": req.get("params").cloned(),
    })
}

#[tokio::test]
async fn ws_request_round_trip() {
    let url = spawn_server(
        Arc::new(|req| {
            // Synchronous handler; build the response from the request.
            futures::executor::block_on(echo_handler(req))
        }),
        vec![],
    )
    .await;

    let t = WebSocketTransport::connect(&url).await.unwrap();
    let result = t
        .request("hello", serde_json::json!({"name": "tako"}))
        .await
        .unwrap();
    assert_eq!(result["method"], "hello");
    assert_eq!(result["params"]["name"], "tako");
    t.close().await.unwrap();
}

#[tokio::test]
async fn ws_concurrent_requests_demux_by_id() {
    let url = spawn_server(
        Arc::new(|req| {
            // Echo back the params verbatim — each request should get its
            // own params back, demonstrating demux by id works.
            req.get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        }),
        vec![],
    )
    .await;

    let t = WebSocketTransport::connect(&url).await.unwrap();
    let t = Arc::new(t);
    let futures: Vec<_> = (0..10)
        .map(|i| {
            let t = Arc::clone(&t);
            tokio::spawn(async move {
                t.request("ping", serde_json::json!({"i": i}))
                    .await
                    .unwrap()
            })
        })
        .collect();
    let results = join_all(futures).await;
    for (i, r) in results.into_iter().enumerate() {
        let val = r.unwrap();
        assert_eq!(val["i"], i);
    }
}

#[tokio::test]
async fn ws_notifications_are_broadcast() {
    let url = spawn_server(
        Arc::new(|_| serde_json::Value::Null),
        vec![
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "log",
                "params": {"level": "info", "msg": "hello"}
            }),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "tick",
                "params": {"n": 1}
            }),
        ],
    )
    .await;

    let t = WebSocketTransport::connect(&url).await.unwrap();
    let mut stream = t.notifications().await;
    // Give the server a moment to push the notifications.
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
    assert_eq!(collected[0]["method"], "log");
    assert_eq!(collected[1]["method"], "tick");
}

#[tokio::test]
async fn ws_connect_error_on_bad_url() {
    // Bind a TCP port that doesn't speak WebSocket; the WS handshake
    // should fail and we expect a Transport error rather than a panic.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}");
    // Drop incoming TCP connections without speaking WS.
    tokio::spawn(async move {
        loop {
            let _ = listener.accept().await;
            // immediately drop
        }
    });
    // Connect from a different TCP socket — there's no WS server, so the
    // handshake fails.
    drop(TcpStream::connect(&addr).await.unwrap());
    let res = WebSocketTransport::connect(&url).await;
    assert!(res.is_err(), "expected connect to fail");
}
