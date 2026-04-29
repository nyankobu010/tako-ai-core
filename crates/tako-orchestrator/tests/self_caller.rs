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

// ---------------------------------------------------------------------------
// Phase 6.C — LlmJudgeGuard budget wiring.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn llm_judge_guard_with_budget_records_judge_usage() {
    use std::collections::BTreeMap;
    use tako_core::Budget;
    use tako_orchestrator::LlmJudgeGuard;
    use tako_runtime::{BudgetBackend, BudgetTracker, InMemoryBudgetBackend};

    // Judge replies with "0.9" and reports usage of (4, 2). The guard's
    // own budget tracker should record those tokens against the
    // tenant id passed through the principal.
    let judge = Arc::new(FakeProvider::new(
        "fake:judge",
        vec![ChatResponse {
            message: Message::assistant("0.9"),
            finish_reason: FinishReason::Stop,
            usage: Usage {
                input_tokens: 4,
                output_tokens: 2,
            },
            raw: Default::default(),
        }],
    ));
    let backend = Arc::new(InMemoryBudgetBackend::new());
    let tracker = Arc::new(BudgetTracker::new(
        Arc::clone(&backend) as Arc<dyn BudgetBackend>,
        Budget::default(),
    ));

    let guard = LlmJudgeGuard::new(judge.clone(), "rate from 0 to 1").with_budget(tracker);

    let principal = Principal {
        tenant_id: "tenant-judge".into(),
        user_id: "u".into(),
        roles: vec![],
        trace_id: None,
        metadata: BTreeMap::new(),
    };
    let score = guard
        .evaluate(&principal, "candidate answer")
        .await
        .unwrap();
    assert!((score - 0.9).abs() < 1e-3);
    assert_eq!(judge.calls.load(Ordering::SeqCst), 1);

    let usage = backend.current_usage("tenant-judge").await.unwrap();
    assert_eq!(usage.tokens_today, 6);
}

// ---------------------------------------------------------------------------
// Phase 7.B — native SelfCaller::stream.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_passes_through_when_confident() {
    use futures::StreamExt;
    use tako_orchestrator::OrchEvent;

    // Confidence guard always returns 1.0 → no recursion. Exactly one
    // outer Final event reaches the caller.
    let provider = Arc::new(FakeProvider::new("fake:p", vec![assistant("the answer")]));
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
    let mut stream = sc
        .stream(&Principal::anonymous(), OrchInput::from_user("hi"))
        .await;
    let mut finals = 0;
    let mut last_text = String::new();
    while let Some(ev) = stream.next().await {
        if let OrchEvent::Final { output } = ev.unwrap() {
            finals += 1;
            last_text = output.text;
        }
    }
    assert_eq!(finals, 1);
    assert_eq!(last_text, "the answer");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stream_recurses_to_max_depth_when_guard_rejects() {
    use futures::StreamExt;
    use tako_orchestrator::OrchEvent;

    // Constantly low confidence → SelfCaller hits max_depth=2 and emits
    // exactly one outer Final, holding the LAST inner output's text.
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
    let mut stream = sc
        .stream(&Principal::anonymous(), OrchInput::from_user("solve"))
        .await;
    let mut finals = 0;
    let mut step_starts = 0;
    let mut last_text = String::new();
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            OrchEvent::Final { output } => {
                finals += 1;
                last_text = output.text;
            }
            OrchEvent::StepStart { .. } => {
                step_starts += 1;
            }
            _ => {}
        }
    }
    assert_eq!(finals, 1, "outer stream must yield exactly one Final");
    assert_eq!(last_text, "attempt 2 (low)");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    // Each inner SingleAgent stream emits a StepStart per inner step;
    // we ran 3 inner orchestrator invocations of 1 step each, so the
    // outer stream forwards 3 StepStarts.
    assert_eq!(step_starts, 3);
}

#[tokio::test]
async fn stream_yields_inner_assistant_text_before_final() {
    use futures::StreamExt;
    use tako_orchestrator::OrchEvent;

    // SingleAgent::stream falls back to chat() + a synthetic
    // AssistantText event for non-streaming providers, then Final. The
    // outer stream must forward the AssistantText before yielding its
    // own Final.
    let provider = Arc::new(FakeProvider::new(
        "fake:p",
        vec![assistant("the answer is 42")],
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
        .max_depth(3)
        .min_confidence(0.5)
        .confidence(Arc::new(ConstantConfidence(1.0)))
        .build()
        .unwrap();
    let mut stream = sc
        .stream(&Principal::anonymous(), OrchInput::from_user("hi"))
        .await;

    let mut events: Vec<OrchEvent> = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev.unwrap());
    }

    let mut saw_text = false;
    let mut saw_final = false;
    for ev in &events {
        match ev {
            OrchEvent::AssistantText { delta, .. } => {
                assert!(
                    !saw_final,
                    "AssistantText must not arrive after the outer Final"
                );
                if delta.contains("42") {
                    saw_text = true;
                }
            }
            OrchEvent::Final { .. } => {
                saw_final = true;
            }
            _ => {}
        }
    }
    assert!(saw_text, "expected an AssistantText carrying the answer");
    assert!(saw_final, "expected exactly one outer Final");
}
