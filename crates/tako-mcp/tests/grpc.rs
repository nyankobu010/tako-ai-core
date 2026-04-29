//! End-to-end GrpcTransport tests against an in-process tonic server.
#![cfg(feature = "grpc")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::StreamExt;
use futures::{Stream, future::join_all};
use tako_core::McpTransport;
use tako_mcp::GrpcTransport;
use tako_mcp::transport::grpc::proto::{
    Frame,
    mcp_bridge_server::{McpBridge, McpBridgeServer},
};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{Request, Response, Status, Streaming, transport::Server};

type RpcHandler = dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync + 'static;

struct McpBridgeImpl {
    handler: Arc<RpcHandler>,
    notifications: Vec<serde_json::Value>,
}

#[tonic::async_trait]
impl McpBridge for McpBridgeImpl {
    type OpenStream = std::pin::Pin<Box<dyn Stream<Item = Result<Frame, Status>> + Send + 'static>>;

    async fn open(
        &self,
        request: Request<Streaming<Frame>>,
    ) -> Result<Response<Self::OpenStream>, Status> {
        let mut inbound = request.into_inner();
        let handler = Arc::clone(&self.handler);
        let notifications = self.notifications.clone();
        let (tx, rx) = mpsc::channel::<Result<Frame, Status>>(64);

        // Push canned notifications immediately on connect.
        for n in notifications {
            let payload = serde_json::to_vec(&n).unwrap();
            tx.send(Ok(Frame { json: payload })).await.unwrap();
        }

        tokio::spawn(async move {
            while let Some(frame) = inbound.next().await {
                let frame = match frame {
                    Ok(f) => f,
                    Err(_) => break,
                };
                let req: serde_json::Value = match serde_json::from_slice(&frame.json) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let id = req.get("id").cloned();
                if id.is_none() {
                    // Notification — no response.
                    continue;
                }
                let result = (handler)(req);
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                });
                let payload = serde_json::to_vec(&resp).unwrap();
                if tx.send(Ok(Frame { json: payload })).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

/// Spin up a one-connection in-process gRPC server bound to an
/// ephemeral port. Returns `http://addr` for `GrpcTransport::connect`.
async fn spawn_server(handler: Arc<RpcHandler>, notifications: Vec<serde_json::Value>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");

    let svc = McpBridgeServer::new(McpBridgeImpl {
        handler,
        notifications,
    });

    tokio::spawn(async move {
        Server::builder()
            .add_service(svc)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    // Give the server a tick to start accepting before the client dials.
    tokio::time::sleep(Duration::from_millis(20)).await;

    url
}

fn echo_handler(req: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "method": req.get("method").cloned(),
        "params": req.get("params").cloned(),
    })
}

#[tokio::test]
async fn grpc_request_round_trip() {
    let url = spawn_server(Arc::new(echo_handler), vec![]).await;

    let t = GrpcTransport::connect(&url).await.unwrap();
    let result = t
        .request("hello", serde_json::json!({"name": "tako"}))
        .await
        .unwrap();
    assert_eq!(result["method"], "hello");
    assert_eq!(result["params"]["name"], "tako");
}

#[tokio::test]
async fn grpc_concurrent_requests_demux_by_id() {
    let url = spawn_server(Arc::new(echo_handler), vec![]).await;
    let t = GrpcTransport::connect(&url).await.unwrap();

    let futures = (0..10).map(|i| {
        let t = t.clone();
        async move {
            t.request("ping", serde_json::json!({"i": i}))
                .await
                .unwrap()
        }
    });
    let results = join_all(futures).await;

    for (i, r) in results.iter().enumerate() {
        assert_eq!(r["method"], "ping");
        assert_eq!(r["params"]["i"], i as i64);
    }
}

#[tokio::test]
async fn grpc_notifications_are_broadcast() {
    let url = spawn_server(
        Arc::new(echo_handler),
        vec![
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {"step": 1}
            }),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {"step": 2}
            }),
        ],
    )
    .await;

    let t = GrpcTransport::connect(&url).await.unwrap();
    let mut stream = t.notifications().await;

    // First two events should be the canned notifications, in order.
    let first = tokio::time::timeout(Duration::from_millis(500), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first["method"], "notifications/progress");
    assert_eq!(first["params"]["step"], 1);

    let second = tokio::time::timeout(Duration::from_millis(500), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(second["method"], "notifications/progress");
    assert_eq!(second["params"]["step"], 2);
}

#[tokio::test]
async fn grpc_connect_error_on_bad_endpoint() {
    // Bind a plain TCP listener that doesn't speak gRPC; connecting
    // should fail with a Transport error rather than hang.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener); // free the port so the dial fails outright

    let url = format!("http://{addr}");
    let result = GrpcTransport::connect(&url).await;
    assert!(matches!(result, Err(tako_core::TakoError::Transport(_))));
}
