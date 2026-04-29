//! Route handlers for the OpenAI-compatible compat server.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Json};
use futures::stream::StreamExt;
use serde_json::json;
use tako_orchestrator::{OrchInput, Orchestrator};

use crate::auth::AuthResolver;
use crate::openai::{OaChatRequest, from_openai_request, models_list, to_openai_response};
use crate::sse::{event_to_payloads, new_chunk_id};

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
    let principal = match resolve_principal(&headers, state.auth.as_ref()).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    if body.stream {
        return chat_completions_stream(state, principal, body).await;
    }

    let model = body.model.clone();
    let req = from_openai_request(body);
    let prompt = extract_user_prompt(&req).unwrap_or_default();
    let result = state
        .orch
        .run(&principal, OrchInput::from_user(prompt))
        .await;
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

async fn chat_completions_stream(
    state: Arc<AppState>,
    principal: tako_core::Principal,
    body: OaChatRequest,
) -> axum::response::Response {
    use tako_orchestrator::OrchEvent;

    let model = body.model.clone();
    let chunk_id = new_chunk_id();
    let req = from_openai_request(body);
    let prompt = extract_user_prompt(&req).unwrap_or_default();

    let orch = state.orch.clone();
    let principal_for_run = principal.clone();
    let prompt_for_run = prompt.clone();

    // Try the orchestrator's native streaming first; fall back to a
    // single-chunk emulation backed by `run()` for orchestrators that
    // don't yet implement event streaming. The wire format is identical
    // either way (chunks + `data: [DONE]`), so the OpenAI SDK can't tell
    // the difference.
    let mut native = orch
        .stream(&principal, OrchInput::from_user(prompt))
        .await;
    let head = native.next().await;
    let event_stream: futures::stream::BoxStream<'static, Result<OrchEvent, tako_core::TakoError>> =
        match head {
            Some(Err(_)) => {
                // Native stream not implemented — emulate by running and
                // emitting one assistant-text chunk + Final.
                let orch_for_run = orch.clone();
                futures::stream::once(async move {
                    match orch_for_run
                        .run(&principal_for_run, OrchInput::from_user(prompt_for_run))
                        .await
                    {
                        Ok(out) => Ok(OrchEvent::Final {
                            output: Box::new(out),
                        }),
                        Err(e) => Err(e),
                    }
                })
                .flat_map(emulated_text_then_final)
                .boxed()
            }
            Some(Ok(first)) => futures::stream::once(async move { Ok(first) })
                .chain(native)
                .boxed(),
            None => futures::stream::empty().boxed(),
        };

    let sse_stream = event_stream
        .flat_map(move |item| {
            let id = chunk_id.clone();
            let model = model.clone();
            let payloads: Vec<Result<SseEvent, Infallible>> = match item {
                Ok(ev) => event_to_payloads(&ev, &id, &model)
                    .into_iter()
                    .map(|p| Ok(SseEvent::default().data(p)))
                    .collect(),
                Err(e) => {
                    let frame = json!({
                        "error": {"message": e.to_string(), "type": "internal_error"}
                    });
                    vec![Ok(SseEvent::default().data(frame.to_string()))]
                }
            };
            futures::stream::iter(payloads)
        })
        .chain(futures::stream::once(async {
            Ok(SseEvent::default().data("[DONE]"))
        }));

    Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// When the orchestrator can't natively stream, expand a final
/// `OrchOutput` into an `AssistantText` chunk (so the SDK sees content
/// arrive) plus a `Final` event (so finish_reason + usage are reported).
fn emulated_text_then_final(
    item: Result<tako_orchestrator::OrchEvent, tako_core::TakoError>,
) -> futures::stream::BoxStream<'static, Result<tako_orchestrator::OrchEvent, tako_core::TakoError>>
{
    use tako_orchestrator::OrchEvent;
    match item {
        Err(e) => futures::stream::once(async move { Err(e) }).boxed(),
        Ok(OrchEvent::Final { output }) => {
            let text = output.text.clone();
            let final_evt = OrchEvent::Final { output };
            let mut events: Vec<Result<OrchEvent, tako_core::TakoError>> = Vec::new();
            if !text.is_empty() {
                events.push(Ok(OrchEvent::AssistantText { step: 0, delta: text }));
            }
            events.push(Ok(final_evt));
            futures::stream::iter(events).boxed()
        }
        Ok(other) => futures::stream::once(async move { Ok(other) }).boxed(),
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
