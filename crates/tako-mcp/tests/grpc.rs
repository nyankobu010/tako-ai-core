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

// ---------------------------------------------------------------------------
// mTLS coverage (Phase 5.B).
//
// Generate a fresh CA + server cert + client cert at test time using
// `rcgen` and run the same JSON-RPC echo round-trip across an mTLS-secured
// in-process tonic server. No fixtures committed.
// ---------------------------------------------------------------------------

mod mtls {
    use super::{McpBridgeImpl, RpcHandler, echo_handler};
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
    };
    use std::net::SocketAddr;
    use std::sync::{Arc, Once};
    use std::time::Duration;

    /// Both `aws-lc-rs` (via rcgen) and `ring` (via tonic) end up linked
    /// in this test binary, so rustls 0.23 can't auto-pick a default
    /// crypto provider. Install one explicitly, exactly once.
    fn ensure_crypto_provider() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        });
    }
    use tako_core::McpTransport;
    use tako_mcp::GrpcTransport;
    use tako_mcp::transport::grpc::proto::mcp_bridge_server::McpBridgeServer;
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::{Identity, Server, ServerTlsConfig};

    struct CertSet {
        ca_pem: String,
        server_cert_pem: String,
        server_key_pem: String,
        client_cert_pem: String,
        client_key_pem: String,
    }

    fn build_certs(server_san: &str) -> CertSet {
        ensure_crypto_provider();
        // CA
        let ca_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut ca_params = CertificateParams::default();
        ca_params.distinguished_name = DistinguishedName::new();
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "tako-test-ca");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca_cert = ca_params.self_signed(&ca_kp).unwrap();
        let ca_pem = ca_cert.pem();
        let ca_issuer: Issuer<'_, _> = Issuer::from_params(&ca_params, &ca_kp);

        // Server leaf (signed by CA)
        let server_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut server_params = CertificateParams::default();
        server_params
            .distinguished_name
            .push(DnType::CommonName, server_san);
        server_params.subject_alt_names = vec![SanType::DnsName(server_san.try_into().unwrap())];
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let server_cert = server_params.signed_by(&server_kp, &ca_issuer).unwrap();
        let server_cert_pem = server_cert.pem();
        let server_key_pem = server_kp.serialize_pem();

        // Client leaf (signed by CA)
        let client_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut client_params = CertificateParams::default();
        client_params
            .distinguished_name
            .push(DnType::CommonName, "tako-test-client");
        client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let client_cert = client_params.signed_by(&client_kp, &ca_issuer).unwrap();
        let client_cert_pem = client_cert.pem();
        let client_key_pem = client_kp.serialize_pem();

        CertSet {
            ca_pem,
            server_cert_pem,
            server_key_pem,
            client_cert_pem,
            client_key_pem,
        }
    }

    async fn spawn_mtls_server(
        certs: &CertSet,
        handler: Arc<RpcHandler>,
        require_client_cert: bool,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let url = format!("https://{addr}");

        let svc = McpBridgeServer::new(McpBridgeImpl {
            handler,
            notifications: vec![],
        });

        let identity = Identity::from_pem(
            certs.server_cert_pem.as_bytes(),
            certs.server_key_pem.as_bytes(),
        );
        let mut tls = ServerTlsConfig::new().identity(identity);
        if require_client_cert {
            tls = tls.client_ca_root(tonic::transport::Certificate::from_pem(
                certs.ca_pem.as_bytes(),
            ));
        }

        tokio::spawn(async move {
            Server::builder()
                .tls_config(tls)
                .unwrap()
                .add_service(svc)
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        tokio::time::sleep(Duration::from_millis(40)).await;
        url
    }

    #[tokio::test]
    async fn mtls_round_trip_with_client_cert() {
        let san = "localhost";
        let certs = build_certs(san);
        let url = spawn_mtls_server(&certs, Arc::new(echo_handler), true).await;

        let t = GrpcTransport::connect_with_tls(
            &url,
            certs.ca_pem.as_bytes(),
            Some(certs.client_cert_pem.as_bytes()),
            Some(certs.client_key_pem.as_bytes()),
            Some(san),
        )
        .await
        .unwrap();
        let result = t
            .request("hello", serde_json::json!({"name": "tako"}))
            .await
            .unwrap();
        assert_eq!(result["method"], "hello");
        assert_eq!(result["params"]["name"], "tako");
    }

    #[tokio::test]
    async fn mtls_rejects_missing_client_cert() {
        let san = "localhost";
        let certs = build_certs(san);
        let url = spawn_mtls_server(&certs, Arc::new(echo_handler), true).await;

        // No client cert / key — server requires one, so the connection
        // must fail (either at handshake or as soon as we try to issue
        // the first request).
        let conn =
            GrpcTransport::connect_with_tls(&url, certs.ca_pem.as_bytes(), None, None, Some(san))
                .await;
        let err_at_request = match conn {
            Err(_) => true,
            Ok(t) => t.request("hello", serde_json::json!({})).await.is_err(),
        };
        assert!(
            err_at_request,
            "expected mTLS handshake or first-request failure when client cert is missing"
        );
    }

    #[tokio::test]
    async fn ca_only_round_trip_without_client_cert() {
        // Server does NOT require a client cert; we still want to use a
        // custom CA bundle (replacing the default webpki-roots store).
        let san = "localhost";
        let certs = build_certs(san);
        let url = spawn_mtls_server(&certs, Arc::new(echo_handler), false).await;

        let t =
            GrpcTransport::connect_with_tls(&url, certs.ca_pem.as_bytes(), None, None, Some(san))
                .await
                .unwrap();
        let result = t
            .request("hello", serde_json::json!({"name": "tako"}))
            .await
            .unwrap();
        assert_eq!(result["method"], "hello");
    }

    #[tokio::test]
    async fn mismatched_client_cert_and_key_rejected_eagerly() {
        // The verifier must reject a half-pair (cert without key) up front
        // so the failure surfaces synchronously instead of mid-handshake.
        let san = "localhost";
        let certs = build_certs(san);
        let url = spawn_mtls_server(&certs, Arc::new(echo_handler), false).await;

        let err = GrpcTransport::connect_with_tls(
            &url,
            certs.ca_pem.as_bytes(),
            Some(certs.client_cert_pem.as_bytes()),
            None,
            Some(san),
        )
        .await
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("client_cert_pem and client_key_pem"),
            "got: {msg}"
        );
    }
}
