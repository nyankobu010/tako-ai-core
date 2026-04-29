//! Route handlers for the OpenAI-compatible compat server.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use serde_json::json;
use tako_orchestrator::{OrchInput, Orchestrator};

use crate::auth::AuthResolver;
use crate::openai::{OaChatRequest, from_openai_request, models_list, to_openai_response};

pub(crate) struct AppState {
    pub orch: Arc<dyn Orchestrator>,
    pub auth: Arc<dyn AuthResolver>,
    pub models: Vec<String>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("models", &self.models)
            .finish_non_exhaustive()
    }
}

pub(crate) async fn healthz() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

pub(crate) async fn readyz() -> impl IntoResponse {
    Json(json!({"status": "ready"}))
}

pub(crate) async fn list_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::to_value(models_list(&state.models)).unwrap_or(json!({})))
}

pub(crate) async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<OaChatRequest>,
) -> impl IntoResponse {
    if body.stream {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({"error": {"message": "streaming is Phase 2.5", "type": "not_implemented"}})),
        )
            .into_response();
    }

    let principal = match resolve_principal(&headers, state.auth.as_ref()).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let model = body.model.clone();
    let req = from_openai_request(body);
    let prompt = extract_user_prompt(&req).unwrap_or_default();
    let result = state.orch.run(&principal, OrchInput::from_user(prompt)).await;
    match result {
        Ok(out) => {
            let resp = to_openai_response(
                model,
                tako_core::ChatResponse {
                    message: out.message,
                    finish_reason: tako_core::FinishReason::Stop,
                    usage: out.usage,
                    raw: Default::default(),
                },
            );
            Json(serde_json::to_value(resp).unwrap_or(json!({}))).into_response()
        }
        Err(tako_core::TakoError::PolicyDenied(reason)) => (
            StatusCode::FORBIDDEN,
            Json(json!({"error": {"message": reason, "type": "policy_denied"}})),
        )
            .into_response(),
        Err(tako_core::TakoError::BudgetExhausted(reason)) => (
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({"error": {"message": reason, "type": "budget_exhausted"}})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"message": e.to_string(), "type": "internal_error"}})),
        )
            .into_response(),
    }
}

async fn resolve_principal(
    headers: &HeaderMap,
    auth: &dyn AuthResolver,
) -> Result<tako_core::Principal, axum::response::Response> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_owned);
    let Some(token) = bearer else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"message": "missing Authorization: Bearer", "type": "auth"}})),
        )
            .into_response());
    };
    auth.resolve(&token).await.map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"message": e.to_string(), "type": "auth"}})),
        )
            .into_response()
    })
}

fn extract_user_prompt(req: &tako_core::ChatRequest) -> Option<String> {
    req.messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, tako_core::Role::User))
        .and_then(|m| {
            m.content
                .iter()
                .find_map(tako_core::ContentPart::as_text)
                .map(str::to_owned)
        })
}
