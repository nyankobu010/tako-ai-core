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

// ---------------------------------------------------------------------------
// Phase 8.B — native AbMcts::stream
// ---------------------------------------------------------------------------

mod stream {
    //! Phase 8.B end-to-end tests for `AbMcts::stream`.
    //!
    //! Exercises the streamed event ordering: per iteration, exactly
    //! one [`OrchEvent::StepStart`] arrives, the rollout's full text
    //! is delivered as a single `AssistantText` delta, and a
    //! `VerifierScore` carrying the verifier's float on `[0, 1]`
    //! arrives *after* the text. The stream ends with exactly one
    //! terminal `Final` event constructed from the highest-scored
    //! leaf.

    use super::*;
    use futures::StreamExt;
    use tako_orchestrator::OrchEvent;

    #[tokio::test]
    async fn stream_emits_step_text_score_then_final() {
        let provider = Arc::new(
            FakeProvider::new("fake:p", vec![assistant("first rollout text")]).with_repeat(),
        );
        let mcts = AbMcts::builder()
            .provider(provider.clone())
            .verifier(Arc::new(AlwaysScore(0.5)))
            .max_iterations(3)
            .max_steps_per_rollout(1)
            .min_confidence(0.95) // never trips
            .build()
            .unwrap();

        let mut stream = mcts
            .stream(&Principal::anonymous(), OrchInput::from_user("go"))
            .await;

        let mut events: Vec<OrchEvent> = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev.unwrap());
        }

        // Exactly 3 iterations × (StepStart + AssistantText + VerifierScore)
        // = 9 events, plus 1 terminal Final = 10 total.
        assert_eq!(events.len(), 10, "got events: {events:?}");

        // First three events are iteration 0's bundle, in order.
        match &events[0] {
            OrchEvent::StepStart { step } => assert_eq!(*step, 0),
            other => panic!("expected StepStart at index 0, got {other:?}"),
        }
        match &events[1] {
            OrchEvent::AssistantText { step, delta } => {
                assert_eq!(*step, 0);
                assert_eq!(delta, "first rollout text");
            }
            other => panic!("expected AssistantText at index 1, got {other:?}"),
        }
        match &events[2] {
            OrchEvent::VerifierScore {
                step,
                branch: _,
                score,
            } => {
                assert_eq!(*step, 0);
                assert!((score - 0.5).abs() < 1e-3);
            }
            other => panic!("expected VerifierScore at index 2, got {other:?}"),
        }
        // Last event is the terminal Final.
        match events.last().unwrap() {
            OrchEvent::Final { output } => {
                assert_eq!(output.text, "first rollout text");
            }
            other => panic!("expected terminal Final, got {other:?}"),
        }
        // Provider was called exactly max_iterations times (one chat
        // per non-streaming rollout, max_steps_per_rollout=1).
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    }

    /// Verifier scores arrive *after* the rollout's AssistantText for
    /// the same iteration, never before. This is the key ordering
    /// guarantee callers rely on (you can't score text you haven't
    /// seen).
    #[tokio::test]
    async fn stream_verifier_scores_arrive_after_assistant_text() {
        let provider =
            Arc::new(FakeProvider::new("fake:p", vec![assistant("rollout body")]).with_repeat());
        let mcts = AbMcts::builder()
            .provider(provider)
            .verifier(Arc::new(AlwaysScore(0.42)))
            .max_iterations(2)
            .max_steps_per_rollout(1)
            .min_confidence(0.95)
            .build()
            .unwrap();

        let mut stream = mcts
            .stream(&Principal::anonymous(), OrchInput::from_user("hi"))
            .await;

        // For each iteration, track the indices of the AssistantText
        // and VerifierScore. The score's position must be > the
        // text's position (and adjacent in this fixture).
        let mut text_idx_by_step: std::collections::HashMap<u32, usize> =
            std::collections::HashMap::new();
        let mut score_idx_by_step: std::collections::HashMap<u32, usize> =
            std::collections::HashMap::new();
        let mut idx = 0usize;
        while let Some(ev) = stream.next().await {
            match ev.unwrap() {
                OrchEvent::AssistantText { step, .. } => {
                    text_idx_by_step.insert(step, idx);
                }
                OrchEvent::VerifierScore { step, .. } => {
                    score_idx_by_step.insert(step, idx);
                }
                _ => {}
            }
            idx += 1;
        }
        for step in 0..2u32 {
            let ti = text_idx_by_step[&step];
            let si = score_idx_by_step[&step];
            assert!(si > ti, "step {step}: score at {si} not after text at {ti}");
        }
    }

    /// `min_confidence` early-stop must end the stream after the
    /// rollout that crosses the threshold — emitting exactly one
    /// Final right after that rollout's VerifierScore.
    #[tokio::test]
    async fn stream_early_stops_on_min_confidence() {
        let provider =
            Arc::new(FakeProvider::new("fake:p", vec![assistant("good enough")]).with_repeat());
        let mcts = AbMcts::builder()
            .provider(provider.clone())
            .verifier(Arc::new(AlwaysScore(0.99)))
            .max_iterations(10)
            .max_steps_per_rollout(1)
            .min_confidence(0.5)
            .build()
            .unwrap();

        let mut stream = mcts
            .stream(&Principal::anonymous(), OrchInput::from_user("go"))
            .await;

        let mut events: Vec<OrchEvent> = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev.unwrap());
        }

        // First-iteration triple + terminal Final = 4 events. The
        // remaining 9 iterations of max_iterations=10 are skipped
        // because score=0.99 ≥ min_confidence=0.5 on iteration 0.
        assert_eq!(
            events.len(),
            4,
            "expected early-stop after one rollout: {events:?}"
        );
        assert!(matches!(events.last(), Some(OrchEvent::Final { .. })));
        // Provider was called exactly once.
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}

mod branch_routing {
    //! Phase 9.D — AB-MCTS router-driven branch expansion tests.
    //!
    //! Each test exercises the optional router that picks the provider
    //! for a single rollout (one branch expansion). Without a router,
    //! every rollout uses the primary provider — this is the
    //! backwards-compatibility regression guard.

    use super::*;
    use tako_core::{Router, RoutingDecision};

    /// Toggle router: alternates between provider 0 and provider 1
    /// across calls. Lets the test assert that AB-MCTS exercises both
    /// providers across branches without depending on the prompt
    /// content. Built around an `AtomicUsize` so the trait method
    /// stays `&self`-immutable.
    struct ToggleRouter {
        ids: Vec<String>,
        next: AtomicUsize,
    }

    impl ToggleRouter {
        fn new(ids: Vec<&str>) -> Self {
            Self {
                ids: ids.into_iter().map(String::from).collect(),
                next: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Router for ToggleRouter {
        async fn route(
            &self,
            _principal: &Principal,
            _req: &ChatRequest,
            _candidates: &[String],
        ) -> Result<RoutingDecision, TakoError> {
            let i = self.next.fetch_add(1, Ordering::SeqCst);
            let id = self.ids[i % self.ids.len()].clone();
            Ok(RoutingDecision {
                provider_id: id,
                confidence: 1.0,
                reason: None,
            })
        }
    }

    /// Always-fail router: surfaces a router error to verify the
    /// caller propagates it rather than swallowing.
    struct FailingRouter;

    #[async_trait]
    impl Router for FailingRouter {
        async fn route(
            &self,
            _principal: &Principal,
            _req: &ChatRequest,
            _candidates: &[String],
        ) -> Result<RoutingDecision, TakoError> {
            Err(TakoError::Invalid("router unavailable".into()))
        }
    }

    /// With two providers + a toggle router, both providers see at
    /// least one rollout each across `max_iterations=4`. The picked
    /// provider drives every step of one branch's rollout, so each
    /// rollout's `chat` call counts toward exactly one provider.
    #[tokio::test]
    async fn routes_branches_across_two_providers() {
        let p0 =
            Arc::new(FakeProvider::new("fake:fast", vec![assistant("fast rollout")]).with_repeat());
        let p1 =
            Arc::new(FakeProvider::new("fake:deep", vec![assistant("deep rollout")]).with_repeat());
        let router = Arc::new(ToggleRouter::new(vec!["fake:fast", "fake:deep"]));
        let mcts = AbMcts::builder()
            .provider(p0.clone())
            .candidate(p1.clone())
            .router(router)
            .verifier(Arc::new(AlwaysScore(0.4)))
            .max_iterations(4)
            .max_steps_per_rollout(1)
            .min_confidence(0.99)
            .build()
            .unwrap();

        mcts.run(&Principal::anonymous(), OrchInput::from_user("anything"))
            .await
            .unwrap();
        assert!(
            p0.calls.load(Ordering::SeqCst) > 0,
            "primary provider must see at least one rollout",
        );
        assert!(
            p1.calls.load(Ordering::SeqCst) > 0,
            "candidate provider must see at least one rollout",
        );
        assert_eq!(
            p0.calls.load(Ordering::SeqCst) + p1.calls.load(Ordering::SeqCst),
            4,
            "every rollout's single chat call must hit exactly one provider",
        );
    }

    /// Without a router, the candidate is registered but ignored —
    /// every rollout uses the primary. Regression guard for the
    /// "no router → backwards-compatible v0.9.0 behaviour" promise.
    #[tokio::test]
    async fn no_router_uses_primary_only() {
        let p0 = Arc::new(FakeProvider::new("fake:p0", vec![assistant("x")]).with_repeat());
        let p1 = Arc::new(FakeProvider::new("fake:p1", vec![assistant("x")]).with_repeat());
        let mcts = AbMcts::builder()
            .provider(p0.clone())
            .candidate(p1.clone())
            .verifier(Arc::new(AlwaysScore(0.5)))
            .max_iterations(3)
            .max_steps_per_rollout(1)
            .min_confidence(0.99)
            .build()
            .unwrap();

        mcts.run(&Principal::anonymous(), OrchInput::from_user("hi"))
            .await
            .unwrap();
        assert_eq!(p0.calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            p1.calls.load(Ordering::SeqCst),
            0,
            "candidate must not be invoked without a router",
        );
    }

    /// A router error must propagate as a TakoError, not silently
    /// fall back to the primary.
    #[tokio::test]
    async fn router_error_propagates() {
        let p0 = Arc::new(FakeProvider::new("fake:p0", vec![assistant("x")]).with_repeat());
        let mcts = AbMcts::builder()
            .provider(p0)
            .router(Arc::new(FailingRouter))
            .verifier(Arc::new(AlwaysScore(0.5)))
            .max_iterations(1)
            .max_steps_per_rollout(1)
            .build()
            .unwrap();

        let res = mcts
            .run(&Principal::anonymous(), OrchInput::from_user("x"))
            .await;
        let err = res.unwrap_err();
        assert!(
            format!("{err}").contains("router unavailable"),
            "expected router error to propagate, got {err}",
        );
    }
}
