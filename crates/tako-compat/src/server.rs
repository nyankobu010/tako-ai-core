//! Server entry point.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use tako_core::TakoError;
use tako_orchestrator::Orchestrator;

use crate::auth::AuthResolver;
use crate::routes::{AppState, chat_completions, healthz, list_models, readyz};

#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub host: String,
    pub port: u16,
    pub models: Vec<String>,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
            models: vec!["tako-default".into()],
        }
    }
}

/// Serve the OpenAI-compatible HTTP surface backed by `orch`.
///
/// Returns the bound socket address (useful when `port = 0` for tests)
/// alongside a future that runs the server until the listener is closed.
/// Most callers will just `serve_openai(...).await?.1.await?`.
///
/// Auth: every request must carry `Authorization: Bearer <token>`; the
/// token is resolved through `auth` to a `Principal` that flows into
/// `Orchestrator::run`.
pub async fn serve_openai(
    orch: Arc<dyn Orchestrator>,
    auth: Arc<dyn AuthResolver>,
    config: ServeConfig,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), TakoError> {
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| TakoError::Invalid(format!("bind addr: {e}")))?;

    let state = Arc::new(AppState {
        orch,
        auth,
        models: config.models,
    });

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| TakoError::Transport(format!("listen: {e}")))?;
    let bound = listener
        .local_addr()
        .map_err(|e| TakoError::Transport(format!("local_addr: {e}")))?;

    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::warn!(error = %e, "tako-compat serve loop exited");
        }
    });

    Ok((bound, handle))
}
