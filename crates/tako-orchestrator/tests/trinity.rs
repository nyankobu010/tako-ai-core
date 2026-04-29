//! Trinity orchestrator end-to-end tests against scripted FakeProviders.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, FinishReason, LlmProvider, Message,
    Principal, TakoError, Usage,
};
use tako_orchestrator::{OrchInput, Orchestrator, RegexRouter, Trinity};

#[derive(Debug)]
struct FakeProvider {
    id: String,
    capabilities: Capabilities,
    responses: tokio::sync::Mutex<VecDeque<ChatResponse>>,
    calls: AtomicUsize,
}

impl FakeProvider {
    fn new(id: &str, responses: Vec<ChatResponse>) -> Self {
        Self {
            id: id.into(),
            capabilities: Capabilities::default(),
            responses: tokio::sync::Mutex::new(responses.into()),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for FakeProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    async fn chat(&self, _p: &Principal, _r: ChatRequest) -> Result<ChatResponse, TakoError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses.lock().await.pop_front().ok_or_else(|| {
            TakoError::Invalid(format!("FakeProvider({}): out of responses", self.id))
        })
    }
    async fn stream(
        &self,
        _p: &Principal,
        _r: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        Err(TakoError::Invalid("not implemented".into()))
    }
}

fn assistant(text: &str) -> ChatResponse {
    ChatResponse {
        message: Message::assistant(text),
        finish_reason: FinishReason::Stop,
        usage: Usage::default(),
        raw: Default::default(),
    }
}

#[tokio::test]
async fn trinity_routes_code_prompt_to_code_role() {
    let code = Arc::new(FakeProvider::new("fake:code", vec![assistant("CODE OUT")]));
    let math = Arc::new(FakeProvider::new("fake:math", vec![assistant("MATH OUT")]));
    let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("FALLBACK")]));
    let trinity = Trinity::builder()
        .role("code", code.clone())
        .role("math", math.clone())
        .role("fallback", fb.clone())
        .router(Arc::new(RegexRouter::default()))
        .max_steps(2)
        .build()
        .unwrap();

    let result = trinity
        .run(
            &Principal::anonymous(),
            OrchInput::from_user("Write a fn to compute fib"),
        )
        .await
        .unwrap();
    assert_eq!(result.text, "CODE OUT");
    assert_eq!(code.calls.load(Ordering::SeqCst), 1);
    assert_eq!(math.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fb.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn trinity_routes_math_prompt_to_math_role() {
    let code = Arc::new(FakeProvider::new("fake:code", vec![assistant("CODE OUT")]));
    let math = Arc::new(FakeProvider::new("fake:math", vec![assistant("MATH OUT")]));
    let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("FB")]));
    let trinity = Trinity::builder()
        .role("code", code.clone())
        .role("math", math.clone())
        .role("fallback", fb.clone())
        .router(Arc::new(RegexRouter::default()))
        .max_steps(2)
        .build()
        .unwrap();

    let result = trinity
        .run(&Principal::anonymous(), OrchInput::from_user("Solve 2+2"))
        .await
        .unwrap();
    assert_eq!(result.text, "MATH OUT");
    assert_eq!(math.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn trinity_falls_back_for_chitchat() {
    let code = Arc::new(FakeProvider::new("fake:code", vec![assistant("X")]));
    let math = Arc::new(FakeProvider::new("fake:math", vec![assistant("X")]));
    let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("hi back!")]));
    let trinity = Trinity::builder()
        .role("code", code.clone())
        .role("math", math.clone())
        .role("fallback", fb.clone())
        .router(Arc::new(RegexRouter::default()))
        .max_steps(2)
        .build()
        .unwrap();

    let result = trinity
        .run(&Principal::anonymous(), OrchInput::from_user("hello"))
        .await
        .unwrap();
    assert_eq!(result.text, "hi back!");
    assert_eq!(fb.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn trinity_errors_without_router() {
    let res = Trinity::builder()
        .role(
            "code",
            Arc::new(FakeProvider::new("fake:c", vec![assistant("x")])),
        )
        .build();
    assert!(res.is_err());
}

#[tokio::test]
async fn trinity_errors_without_roles() {
    let res = Trinity::builder()
        .router(Arc::new(RegexRouter::default()))
        .build();
    assert!(res.is_err());
}
