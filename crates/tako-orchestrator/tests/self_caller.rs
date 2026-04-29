//! SelfCaller end-to-end tests against scripted FakeProviders.
//!
//! Exercises the bounded recursion DoD ("SelfCaller terminates within
//! `max_depth` on adversarial inputs", spec §18 Phase 3).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, ConfidenceGuard, ConstantConfidence,
    FinishReason, LlmProvider, Message, Principal, TakoError, Usage,
};
use tako_orchestrator::{OrchInput, Orchestrator, RuleBasedGuard, SelfCaller, SingleAgent};

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
async fn self_caller_returns_first_output_when_confidence_passes() {
    // Confidence guard always returns 1.0 → no recursion.
    let provider = Arc::new(FakeProvider::new("fake:p", vec![assistant("first answer")]));
    let inner = Arc::new(
        SingleAgent::builder()
            .provider(provider.clone())
            .max_steps(1)
            .build()
            .unwrap(),
    );
    let sc = SelfCaller::builder()
        .inner(inner)
        .max_depth(3)
        .min_confidence(0.5)
        .confidence(Arc::new(ConstantConfidence(1.0)))
        .build()
        .unwrap();
    let out = sc
        .run(&Principal::anonymous(), OrchInput::from_user("hi"))
        .await
        .unwrap();
    assert_eq!(out.text, "first answer");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn self_caller_terminates_within_max_depth_on_adversarial_input() {
    // Confidence is constantly low → SelfCaller should hit max_depth and
    // return the LAST output without overrunning.
    //
    // max_depth=2 means the inner orchestrator may run up to 3 times
    // (offset 0, 1, 2). The provider scripts exactly 3 responses to
    // prove the run loop respects the cap.
    let provider = Arc::new(FakeProvider::new(
        "fake:p",
        vec![
            assistant("attempt 0 (low)"),
            assistant("attempt 1 (low)"),
            assistant("attempt 2 (low)"),
        ],
    ));
    let inner = Arc::new(
        SingleAgent::builder()
            .provider(provider.clone())
            .max_steps(1)
            .build()
            .unwrap(),
    );
    let sc = SelfCaller::builder()
        .inner(inner)
        .max_depth(2)
        .min_confidence(0.99)
        .confidence(Arc::new(ConstantConfidence(0.0)))
        .build()
        .unwrap();
    let out = sc
        .run(&Principal::anonymous(), OrchInput::from_user("solve"))
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    assert_eq!(out.text, "attempt 2 (low)");
}

#[tokio::test]
async fn self_caller_recurses_until_threshold_met() {
    // First two attempts are short; third is long. RuleBasedGuard with
    // min_chars=20 only accepts the third.
    let provider = Arc::new(FakeProvider::new(
        "fake:p",
        vec![
            assistant("nope"),
            assistant("still nope"),
            assistant("this answer is long enough to satisfy the rule guard"),
        ],
    ));
    let inner = Arc::new(
        SingleAgent::builder()
            .provider(provider.clone())
            .max_steps(1)
            .build()
            .unwrap(),
    );
    let guard: Arc<dyn ConfidenceGuard> = Arc::new(RuleBasedGuard::new(20));
    let sc = SelfCaller::builder()
        .inner(inner)
        .max_depth(5)
        .min_confidence(0.5)
        .confidence(guard)
        .build()
        .unwrap();
    let out = sc
        .run(
            &Principal::anonymous(),
            OrchInput::from_user("explain CRDTs"),
        )
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    assert!(out.text.contains("long enough"));
}
