//! Phase 15.A — `AbMcts::stream` calls `Verifier::evaluate_streaming`
//! per assistant-text delta on each rollout's *cumulative* buffer when
//! the picked provider supports streaming.
//!
//! `Ok(Some(score))` returns from the hook produce intermediate
//! `OrchEvent::VerifierScore { step, branch, score }` events on the
//! same `(step, branch)` as the eventual synthesis-complete final
//! (Phase 8). The default `Ok(None)` impl preserves Phase 8 behaviour
//! byte-for-byte. Non-streaming providers produce zero partials,
//! exactly one full-text `AssistantText` plus one final per rollout —
//! identical to v0.15.0.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tako_core::{
    AlwaysScore, Capabilities, ChatChunk, ChatRequest, ChatResponse, FinishReason, LlmProvider,
    Message, Principal, Router, RoutingDecision, TakoError, Usage, Verifier,
};
use tako_orchestrator::{AbMcts, OrchEvent, OrchInput, Orchestrator};

// ---------------------------------------------------------------------------
// Test fixtures.
// ---------------------------------------------------------------------------

/// Non-streaming scripted provider (`supports_streaming = false`). Used
/// for fallback / baseline tests.
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
            repeat_last: true,
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
        Err(TakoError::Invalid(
            "FakeProvider does not stream in this test".into(),
        ))
    }
}

/// Streaming-capable fake. Emits a fixed series of text deltas, then
/// `End { Stop }`. Repeats the same delta sequence for every call so
/// AB-MCTS can drive multiple rollouts.
#[derive(Debug)]
struct StreamingFake {
    id: String,
    capabilities: Capabilities,
    deltas: Vec<String>,
    calls: AtomicUsize,
}

impl StreamingFake {
    fn new(id: &str, deltas: Vec<&str>) -> Self {
        Self {
            id: id.into(),
            capabilities: Capabilities {
                supports_streaming: true,
                ..Default::default()
            },
            deltas: deltas.into_iter().map(String::from).collect(),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for StreamingFake {
    fn id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    async fn chat(&self, _p: &Principal, _r: ChatRequest) -> Result<ChatResponse, TakoError> {
        // Some AB-MCTS code paths still call chat() (rollout fallback).
        // Surface a deterministic single response so we can also use
        // StreamingFake as a non-failure baseline if needed.
        Err(TakoError::Invalid("StreamingFake.chat not used".into()))
    }
    async fn stream(
        &self,
        _p: &Principal,
        _r: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let deltas = self.deltas.clone();
        let s = async_stream::stream! {
            for d in deltas {
                yield Ok(ChatChunk::Delta { text: Some(d), tool_calls: vec![] });
            }
            yield Ok(ChatChunk::End {
                finish_reason: FinishReason::Stop,
                usage: Usage::default(),
            });
        };
        Ok(Box::pin(s))
    }
}

fn assistant_text(text: &str) -> ChatResponse {
    ChatResponse {
        message: Message::assistant(text),
        finish_reason: FinishReason::Stop,
        usage: Usage::default(),
        raw: Default::default(),
    }
}

/// Counts every `evaluate_streaming` invocation and returns
/// `Ok(Some(0.5))` so each call produces a streaming partial.
/// `score()` (the synthesis-complete final) returns 0.9.
#[derive(Default)]
struct CountingStreamingVerifier {
    streaming_calls: AtomicUsize,
}

#[async_trait]
impl Verifier for CountingStreamingVerifier {
    async fn score(
        &self,
        _principal: &Principal,
        _prompt: &str,
        _output: &str,
    ) -> Result<f32, TakoError> {
        Ok(0.9)
    }

    async fn evaluate_streaming(
        &self,
        _principal: &Principal,
        _partial: &str,
    ) -> Result<Option<f32>, TakoError> {
        self.streaming_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(0.5))
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ab_mcts_stream_emits_per_delta_assistant_text() {
    // 3 deltas per rollout × 2 rollouts. AB-MCTS should yield each
    // delta as its own AssistantText event.
    let provider = Arc::new(StreamingFake::new("fake:s", vec!["fn ", "main() ", "{}\n"]));
    let mcts = AbMcts::builder()
        .provider(provider.clone())
        .verifier(Arc::new(AlwaysScore(0.4)))
        .max_iterations(2)
        .max_steps_per_rollout(1)
        .min_confidence(0.99)
        .build()
        .unwrap();

    let mut stream = mcts
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;

    let mut deltas: Vec<(u32, String)> = Vec::new();
    while let Some(ev) = stream.next().await {
        if let OrchEvent::AssistantText { step, delta } = ev.unwrap() {
            deltas.push((step, delta));
        }
    }

    // 2 rollouts × 3 deltas = 6 AssistantText events total.
    assert_eq!(deltas.len(), 6, "got deltas: {deltas:?}");
    // Each rollout's deltas appear in scripted order.
    for chunk in deltas.chunks(3) {
        assert_eq!(chunk[0].1, "fn ");
        assert_eq!(chunk[1].1, "main() ");
        assert_eq!(chunk[2].1, "{}\n");
    }
}

#[tokio::test]
async fn ab_mcts_stream_emits_per_delta_verifier_score() {
    // 3 deltas per rollout × `CountingStreamingVerifier` → 3 partials
    // per rollout (score=0.5) + 1 final per rollout (score=0.9).
    // 2 rollouts → 6 partials + 2 finals = 8 VerifierScore events.
    let provider = Arc::new(StreamingFake::new("fake:s", vec!["fn ", "main() ", "{}\n"]));
    let v = Arc::new(CountingStreamingVerifier::default());
    let mcts = AbMcts::builder()
        .provider(provider.clone())
        .verifier(v.clone())
        .max_iterations(2)
        .max_steps_per_rollout(1)
        .min_confidence(0.99) // never trips (max final = 0.9)
        .build()
        .unwrap();

    let mut stream = mcts
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;

    let mut scores: Vec<(u32, u32, f32)> = Vec::new();
    while let Some(ev) = stream.next().await {
        if let OrchEvent::VerifierScore {
            step,
            branch,
            score,
        } = ev.unwrap()
        {
            scores.push((step, branch, score));
        }
    }

    let partials: Vec<_> = scores
        .iter()
        .filter(|(_, _, s)| (s - 0.5).abs() < 1e-6)
        .collect();
    let finals: Vec<_> = scores
        .iter()
        .filter(|(_, _, s)| (s - 0.9).abs() < 1e-6)
        .collect();
    assert_eq!(partials.len(), 6, "partials: {partials:?}");
    assert_eq!(finals.len(), 2, "finals: {finals:?}");
    // Verifier hook fired once per delta — 3 deltas × 2 rollouts.
    assert_eq!(v.streaming_calls.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn ab_mcts_stream_partial_and_final_share_branch() {
    // Each rollout's partial VerifierScore events must carry the same
    // `branch` (= leaf_idx) as that rollout's eventual final.
    let provider = Arc::new(StreamingFake::new("fake:s", vec!["alpha", "beta"]));
    let v = Arc::new(CountingStreamingVerifier::default());
    let mcts = AbMcts::builder()
        .provider(provider)
        .verifier(v)
        .max_iterations(2)
        .max_steps_per_rollout(1)
        .min_confidence(0.99)
        .build()
        .unwrap();

    let mut stream = mcts
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;

    let mut events: Vec<OrchEvent> = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev.unwrap());
    }

    // For each iteration (step), all VerifierScore events on that step
    // must share the same branch identity.
    use std::collections::HashMap;
    let mut by_step: HashMap<u32, Vec<u32>> = HashMap::new();
    for ev in &events {
        if let OrchEvent::VerifierScore { step, branch, .. } = ev {
            by_step.entry(*step).or_default().push(*branch);
        }
    }
    assert_eq!(by_step.len(), 2);
    for (step, branches) in &by_step {
        let first = branches[0];
        assert!(
            branches.iter().all(|b| *b == first),
            "step {step}: branches diverged: {branches:?}",
        );
    }
}

#[tokio::test]
async fn ab_mcts_stream_default_evaluate_streaming_no_partials() {
    // `AlwaysScore` does NOT override `evaluate_streaming` (default
    // `Ok(None)`). Even with a streaming provider, only synthesis-
    // complete finals fire — Phase 8 byte-for-byte parity.
    let provider = Arc::new(StreamingFake::new("fake:s", vec!["a", "b", "c"]));
    let mcts = AbMcts::builder()
        .provider(provider)
        .verifier(Arc::new(AlwaysScore(0.42)))
        .max_iterations(2)
        .max_steps_per_rollout(1)
        .min_confidence(0.99)
        .build()
        .unwrap();

    let mut stream = mcts
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;

    let mut scores: Vec<(u32, u32, f32)> = Vec::new();
    while let Some(ev) = stream.next().await {
        if let OrchEvent::VerifierScore {
            step,
            branch,
            score,
        } = ev.unwrap()
        {
            scores.push((step, branch, score));
        }
    }

    // Exactly one final per rollout — no partials.
    assert_eq!(scores.len(), 2, "scores: {scores:?}");
    for (_, _, score) in &scores {
        assert!((score - 0.42).abs() < 1e-6);
    }
}

/// Phase 16.A.1 — bounded mpsc backpressure regression test.
///
/// Drives `AbMcts::stream` through far more deltas (256) than the
/// channel capacity (64) under a `CountingStreamingVerifier` so each
/// delta also produces a `VerifierScore` partial. With the Phase 15.A
/// unbounded channel this exercised pure memory growth; with the Phase
/// 16.A.1 bounded channel the producer (`rollout_static_streaming`)
/// must repeatedly block on `send().await` and resume as the
/// `tokio::select!` consumer drains. The test passes iff every delta
/// is delivered and the streaming-verifier hook fires exactly N times
/// — i.e. backpressure neither drops events nor deadlocks the
/// producer.
#[tokio::test]
async fn ab_mcts_stream_bounded_backpressure_high_delta_count() {
    const N_DELTAS: usize = 256; // 4× the 64-slot bound
    let deltas: Vec<&str> = std::iter::repeat_n("x", N_DELTAS).collect();
    let provider = Arc::new(StreamingFake::new("fake:s", deltas));
    let v = Arc::new(CountingStreamingVerifier::default());
    let mcts = AbMcts::builder()
        .provider(provider.clone())
        .verifier(v.clone())
        .max_iterations(1)
        .max_steps_per_rollout(1)
        .min_confidence(0.99)
        .build()
        .unwrap();

    let mut stream = mcts
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;

    let mut deltas_seen = 0_usize;
    let mut partial_scores = 0_usize;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            OrchEvent::AssistantText { .. } => deltas_seen += 1,
            OrchEvent::VerifierScore { score, .. } if (score - 0.5).abs() < 1e-6 => {
                partial_scores += 1;
            }
            _ => {}
        }
    }

    // Every produced delta crossed the bounded channel without loss.
    assert_eq!(deltas_seen, N_DELTAS, "deltas dropped under backpressure");
    assert_eq!(partial_scores, N_DELTAS, "verifier partials dropped");
    // Streaming hook fired once per delta on the producer side.
    assert_eq!(v.streaming_calls.load(Ordering::SeqCst), N_DELTAS);
}

#[tokio::test]
async fn ab_mcts_stream_non_streaming_fallback_byte_parity() {
    // Non-streaming provider — exactly one full-text AssistantText per
    // rollout, identical to v0.15.0.
    let provider = Arc::new(FakeProvider::new(
        "fake:ns",
        vec![assistant_text("hello world")],
    ));
    let v = Arc::new(CountingStreamingVerifier::default());
    let mcts = AbMcts::builder()
        .provider(provider.clone())
        .verifier(v.clone())
        .max_iterations(2)
        .max_steps_per_rollout(1)
        .min_confidence(0.99)
        .build()
        .unwrap();

    let mut stream = mcts
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;

    let mut deltas: Vec<String> = Vec::new();
    let mut scores: Vec<(u32, u32, f32)> = Vec::new();
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            OrchEvent::AssistantText { delta, .. } => deltas.push(delta),
            OrchEvent::VerifierScore {
                step,
                branch,
                score,
            } => scores.push((step, branch, score)),
            _ => {}
        }
    }

    // Exactly one full-text delta per rollout.
    assert_eq!(deltas.len(), 2);
    for d in &deltas {
        assert_eq!(d, "hello world");
    }
    // No streaming hook calls — provider doesn't stream.
    assert_eq!(v.streaming_calls.load(Ordering::SeqCst), 0);
    // One synthesis-complete final per rollout.
    assert_eq!(scores.len(), 2, "scores: {scores:?}");
    for (_, _, s) in &scores {
        assert!((s - 0.9).abs() < 1e-6);
    }
}

// ---------------------------------------------------------------------------
// Phase 9.D × Phase 15.A — router-driven branch expansion picks the
// right capability path.
// ---------------------------------------------------------------------------

/// Always returns the candidate id at index 1 (the second of
/// `[primary, ...candidates]`).
struct PickSecond {
    target: String,
}

#[async_trait]
impl Router for PickSecond {
    async fn route(
        &self,
        _p: &Principal,
        _r: &ChatRequest,
        _candidates: &[String],
    ) -> Result<RoutingDecision, TakoError> {
        Ok(RoutingDecision {
            provider_id: self.target.clone(),
            confidence: 1.0,
            reason: None,
        })
    }
}

#[tokio::test]
async fn ab_mcts_stream_router_picks_streaming_candidate() {
    // Primary is non-streaming, candidate is streaming. Router picks
    // candidate. Streaming hook fires per delta.
    let primary = Arc::new(FakeProvider::new(
        "fake:primary",
        vec![assistant_text("non-stream")],
    ));
    let candidate = Arc::new(StreamingFake::new("fake:cand", vec!["X1", "X2", "X3"]));
    let v = Arc::new(CountingStreamingVerifier::default());
    let mcts = AbMcts::builder()
        .provider(primary.clone())
        .candidate(candidate.clone())
        .router(Arc::new(PickSecond {
            target: "fake:cand".into(),
        }))
        .verifier(v.clone())
        .max_iterations(1)
        .max_steps_per_rollout(1)
        .min_confidence(0.99)
        .build()
        .unwrap();

    let mut stream = mcts
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;

    let mut deltas: Vec<String> = Vec::new();
    while let Some(ev) = stream.next().await {
        if let OrchEvent::AssistantText { delta, .. } = ev.unwrap() {
            deltas.push(delta);
        }
    }
    // 3 per-delta events from the streaming candidate.
    assert_eq!(deltas, vec!["X1", "X2", "X3"]);
    // 3 streaming-verifier hook calls.
    assert_eq!(v.streaming_calls.load(Ordering::SeqCst), 3);
    // Primary was never invoked.
    assert_eq!(primary.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ab_mcts_stream_router_picks_non_streaming_candidate() {
    // Primary is streaming, candidate is non-streaming. Router picks
    // candidate → fallback to single full-text delta, zero hook calls.
    let primary = Arc::new(StreamingFake::new("fake:primary", vec!["P1", "P2"]));
    let candidate = Arc::new(FakeProvider::new(
        "fake:cand",
        vec![assistant_text("static reply")],
    ));
    let v = Arc::new(CountingStreamingVerifier::default());
    let mcts = AbMcts::builder()
        .provider(primary.clone())
        .candidate(candidate.clone())
        .router(Arc::new(PickSecond {
            target: "fake:cand".into(),
        }))
        .verifier(v.clone())
        .max_iterations(1)
        .max_steps_per_rollout(1)
        .min_confidence(0.99)
        .build()
        .unwrap();

    let mut stream = mcts
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;

    let mut deltas: Vec<String> = Vec::new();
    while let Some(ev) = stream.next().await {
        if let OrchEvent::AssistantText { delta, .. } = ev.unwrap() {
            deltas.push(delta);
        }
    }
    // Single full-text delta — non-streaming fallback.
    assert_eq!(deltas, vec!["static reply"]);
    // No per-delta verifier hook calls.
    assert_eq!(v.streaming_calls.load(Ordering::SeqCst), 0);
    // Primary streaming was never invoked.
    assert_eq!(primary.calls.load(Ordering::SeqCst), 0);
}
