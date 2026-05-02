//! Stdio transport against a tiny in-process JSON-RPC echo server
//! implemented as a shell script.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tako_core::{McpTransport, Principal, ToolSchema};
use tako_mcp::{StdioTransport, ToolRegistry};

/// Spawn a minimal stdio MCP-style server using bash. It echoes one
/// canned `tools/list` response and one `tools/call` response for the
/// "echo" tool, then exits.
fn server_script() -> String {
    r#"
while IFS= read -r line; do
  case "$line" in
    *initialize*)
      echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"0.0.1"}}}'
      ;;
    *tools/list*)
      echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo input.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}'
      ;;
    *tools/call*)
      echo '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"echoed"}]}}'
      ;;
  esac
done
"#
    .to_string()
}

// On Windows, this test relies on `bash` being on PATH (Git Bash on the
// GitHub Actions runner) plus on stdio piping into bash behaving the
// same as on a POSIX shell. In practice the bash subprocess startup +
// line-buffered stdin pipe is slow enough on `windows-latest` that the
// 2-second tokio::time::timeout on the handshake fires before the
// canned `initialize` reply makes it back. The implementation under
// test (`StdioTransport` over async `tokio::process::Child`) is
// OS-agnostic; the bash fixture is what's flaky. macOS + Linux are the
// authoritative coverage for this transport.
#[cfg_attr(
    target_os = "windows",
    ignore = "bash subprocess timing on Git Bash is flaky on windows-latest; macOS + Linux cover the transport contract"
)]
#[tokio::test]
async fn stdio_request_response_roundtrip() {
    let script = server_script();
    let transport: Arc<dyn McpTransport> = Arc::new(
        StdioTransport::spawn("bash", &["-c".into(), script])
            .await
            .unwrap(),
    );

    // We don't strictly need the handshake for the test, but exercising
    // it makes sure initialize+initialized work end-to-end.
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        tako_mcp::handshake(Arc::clone(&transport), tako_mcp::ClientInfo::tako()),
    )
    .await
    .unwrap();

    let registry = ToolRegistry::new();
    let n = registry.discover(Arc::clone(&transport)).await.unwrap();
    assert_eq!(n, 1);

    let schemas: Vec<ToolSchema> = registry.schemas().await;
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "echo");

    let result = registry
        .invoke(&Principal::anonymous(), "echo", json!({"text":"hi"}))
        .await
        .unwrap();
    assert!(result["content"].is_array());

    transport.close().await.unwrap();
}

#[tokio::test]
async fn registry_unknown_tool_errors() {
    let registry = ToolRegistry::new();
    let err = registry
        .invoke(&Principal::anonymous(), "missing", json!({}))
        .await
        .unwrap_err();
    use tako_core::TakoError;
    assert!(matches!(err, TakoError::NotFound(_)));
}
