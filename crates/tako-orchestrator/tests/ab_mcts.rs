//! AB-MCTS orchestrator end-to-end tests against scripted FakeProviders.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::BoxStream;
use tako_core::{
    AlwaysScore, Capabilities, ChatChunk, ChatRequest, ChatResponse, FinishReason, LlmProvider,
    Message, Principal, TakoError, Usage, Verifier,
};
use tako_orchestrator::{AbMcts, OrchInput, Orchestrator, RuleBasedVerifier};

#[derive(Debug)]
struct FakeProvider {
    id: String,
    capabilities: Capabilities,
    responses: tokio::sync::Mutex<VecDeque<ChatResponse>>,
    calls: AtomicUsize,
    repeat_last: bool,
}

impl FakeProvider {
    fn new(id: &str, responses: Vec<ChatResponse>) -> Self {
        Self {
            id: id.into(),
            capabilities: Capabilities::default(),
            responses: tokio::sync::Mutex::new(responses.into()),
            calls: AtomicUsize::new(0),
            repeat_last: false,
        }
    }

    /// When the response queue is exhausted, repeat the most recent
    /// response forever instead of erroring. Useful for AB-MCTS tests
    /// that drive many iterations.
    fn with_repeat(mut self) -> Self {
        self.repeat_last = true;
        self
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
        let mut q = self.responses.lock().await;
        if let Some(front) = q.pop_front() {
            if self.repeat_last {
                q.push_back(front.clone());
            }
            Ok(front)
        } else {
            Err(TakoError::Invalid(format!(
                "FakeProvider({}): out of responses",
                self.id
            )))
        }
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
async fn ab_mcts_terminates_within_max_iterations() {
    // Provider returns the same low-quality response forever; verifier
    // always scores 0.0; we should still terminate after exactly
    // max_iterations rollouts.
    let provider =
        Arc::new(FakeProvider::new("fake:p", vec![assistant("bad answer")]).with_repeat());
    let mcts = AbMcts::builder()
        .provider(provider.clone())
        .verifier(Arc::new(AlwaysScore(0.0)))
        .max_iterations(5)
        .max_steps_per_rollout(1)
        .build()
        .unwrap();

    let result = mcts
        .run(&Principal::anonymous(), OrchInput::from_user("solve me"))
        .await
        .unwrap();
    assert_eq!(result.text, "bad answer");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn ab_mcts_returns_best_leaf() {
    // Two scripted responses: a "good" one and several "bad" ones.
    // Verifier scores >0.9 on the good one and 0.1 on bad. AB-MCTS
    // should explore enough to surface the good rollout as the best
    // leaf and return its text.
    let provider = Arc::new(
        FakeProvider::new(
            "fake:p",
            vec![
                assistant("bad 1"),
                assistant("bad 2"),
                assistant("GREAT answer"),
                assistant("bad 4"),
            ],
        )
        .with_repeat(),
    );

    struct GreatVerifier;
    #[async_trait]
    impl Verifier for GreatVerifier {
        async fn score(
            &self,
            _p: &Principal,
            _prompt: &str,
            output: &str,
        ) -> Result<f32, TakoError> {
            Ok(if output.contains("GREAT") { 0.99 } else { 0.1 })
        }
    }

    let mcts = AbMcts::builder()
        .provider(provider.clone())
        .verifier(Arc::new(GreatVerifier))
        .max_iterations(8)
        .max_steps_per_rollout(1)
        .min_confidence(0.95)
        .build()
        .unwrap();

    let result = mcts
        .run(&Principal::anonymous(), OrchInput::from_user("anything"))
        .await
        .unwrap();
    assert_eq!(result.text, "GREAT answer");
    // Min-confidence early stop should have kicked in well under 8.
    assert!(provider.calls.load(Ordering::SeqCst) <= 8);
}

#[tokio::test]
async fn ab_mcts_errors_without_provider() {
    let res = AbMcts::builder()
        .verifier(Arc::new(AlwaysScore(1.0)))
        .build();
    assert!(res.is_err());
}

#[tokio::test]
async fn ab_mcts_errors_without_verifier() {
    let res = AbMcts::builder()
        .provider(Arc::new(FakeProvider::new("fake:p", vec![assistant("x")])))
        .build();
    assert!(res.is_err());
}

#[tokio::test]
async fn ab_mcts_with_rule_based_verifier() {
    // Rule-based verifier with a min-length rule. Provider always
    // emits a 30-character response — passes.
    let provider = Arc::new(
        FakeProvider::new("fake:p", vec![assistant("a thirty-character output here")])
            .with_repeat(),
    );
    let verifier = Arc::new(RuleBasedVerifier::new(20));
    let mcts = AbMcts::builder()
        .provider(provider.clone())
        .verifier(verifier)
        .max_iterations(3)
        .max_steps_per_rollout(1)
        .min_confidence(0.95)
        .build()
        .unwrap();

    let result = mcts
        .run(&Principal::anonymous(), OrchInput::from_user("anything"))
        .await
        .unwrap();
    assert_eq!(result.text, "a thirty-character output here");
    // Should hit min_confidence (1.0 from rule-based) on the first
    // rollout and terminate.
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}
