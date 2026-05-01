//! Phase 14.A — `Conductor::stream` calls `Verifier::evaluate_streaming`
//! per assistant-text delta on each worker's *cumulative* buffer when
//! the worker provider supports streaming and a verifier is attached.
//!
//! `Ok(Some(score))` returns from the hook produce intermediate
//! `OrchEvent::VerifierScore { step, branch, score }` events on the
//! same `(step, branch)` as the eventual synthesis-complete final
//! (Phase 10.C). The default `Ok(None)` impl preserves Phase 10.C
//! behaviour byte-for-byte. Non-streaming workers produce zero
//! partials, exactly one synthesis-complete final per worker — same
//! shape as a `StaticTokens` baseline.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::json;
use tako_core::{
    AlwaysScore, Capabilities, ChatChunk, ChatRequest, ChatResponse, FinishReason, LlmProvider,
    Message, Principal, TakoError, Usage, Verifier,
};
use tako_orchestrator::{Conductor, OrchEvent, OrchInput, Orchestrator};

// ---------------------------------------------------------------------------
// Test fixtures.
// ---------------------------------------------------------------------------

/// Scripted non-streaming provider — used for the coordinator and for
/// non-streaming-worker regression tests.
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
        Err(TakoError::Invalid(
            "FakeProvider does not stream in this test".into(),
        ))
    }
}

/// Streaming-capable fake. Emits a fixed series of text deltas, then End.
/// Optional per-delta delay simulates concurrent worker completion order.
#[derive(Debug)]
struct StreamingFake {
    id: String,
    capabilities: Capabilities,
    deltas: Vec<String>,
    per_delta_delay: Duration,
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
            per_delta_delay: Duration::ZERO,
        }
    }

    fn with_per_delta_delay(mut self, d: Duration) -> Self {
        self.per_delta_delay = d;
        self
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
        Err(TakoError::Invalid("StreamingFake.chat not used".into()))
    }
    async fn stream(
        &self,
        _p: &Principal,
        _r: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
        let deltas = self.deltas.clone();
        let delay = self.per_delta_delay;
        let s = async_stream::stream! {
            for d in deltas {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
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

fn coord_dispatch(workers: &[(&str, &str)]) -> ChatResponse {
    let plan = json!({
        "thought": "delegating",
        "dispatch": workers.iter().map(|(w, t)| json!({"worker": w, "task": t})).collect::<Vec<_>>(),
        "halt": false,
    });
    assistant_text(&plan.to_string())
}

fn coord_halt(answer: &str) -> ChatResponse {
    let plan = json!({
        "thought": "done",
        "dispatch": [],
        "halt": true,
        "final_answer": answer,
    });
    assistant_text(&plan.to_string())
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
async fn conductor_emits_per_delta_streaming_verifier_scores_per_worker() {
    // Two streaming workers — `code` yields three deltas, `math` yields
    // two. With `CountingStreamingVerifier`, we expect:
    //   - 3 partials @ score 0.5 for branch=1 (code)
    //   - 2 partials @ score 0.5 for branch=2 (math)
    //   - 1 final  @ score 0.9 for branch=1
    //   - 1 final  @ score 0.9 for branch=2
    // Total: 5 partials + 2 finals = 7 VerifierScore events on step=0.
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![
            coord_dispatch(&[("code", "write fib"), ("math", "verify")]),
            coord_halt("done"),
        ],
    ));
    let code = Arc::new(StreamingFake::new(
        "fake:code",
        vec!["fn ", "main() ", "{}\n"],
    ));
    let math = Arc::new(StreamingFake::new("fake:math", vec!["2+", "2=4"]));

    let v = Arc::new(CountingStreamingVerifier::default());
    let cond = Conductor::builder()
        .coordinator(coordinator)
        .worker("code", code)
        .worker("math", math)
        .max_steps(5)
        .verifier(v.clone())
        .build()
        .unwrap();

    let mut stream = cond
        .stream(
            &Principal::anonymous(),
            OrchInput::from_user("plan two workers"),
        )
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

    // 5 partials + 2 finals = 7 total.
    assert_eq!(scores.len(), 7, "got events: {scores:?}");
    let partials: Vec<_> = scores
        .iter()
        .filter(|(_, _, s)| (s - 0.5).abs() < 1e-6)
        .collect();
    let finals: Vec<_> = scores
        .iter()
        .filter(|(_, _, s)| (s - 0.9).abs() < 1e-6)
        .collect();
    assert_eq!(partials.len(), 5, "partial events: {partials:?}");
    assert_eq!(finals.len(), 2, "final events: {finals:?}");
    // Synthesis-complete finals: one per worker, branches 1 and 2.
    let mut final_branches: Vec<u32> = finals.iter().map(|(_, b, _)| *b).collect();
    final_branches.sort();
    assert_eq!(final_branches, vec![1, 2]);
    // Partial branches must be a subset of {1, 2}.
    let mut partial_branches: Vec<u32> = partials.iter().map(|(_, b, _)| *b).collect();
    partial_branches.sort();
    partial_branches.dedup();
    assert_eq!(partial_branches, vec![1, 2]);
    // 3 deltas for `code` (branch=1) + 2 deltas for `math` (branch=2) = 5 hook calls.
    assert_eq!(v.streaming_calls.load(Ordering::SeqCst), 5);
    // All on step 0.
    for (step, _, _) in &scores {
        assert_eq!(*step, 0);
    }
}

#[tokio::test]
async fn conductor_default_verifier_emits_only_final_score_per_worker() {
    // `AlwaysScore` does NOT override `evaluate_streaming` (default
    // `Ok(None)`). Even with streaming workers, only synthesis-complete
    // finals fire — byte-for-byte parity with Phase 10.C.
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![
            coord_dispatch(&[("code", "write fib"), ("math", "verify")]),
            coord_halt("done"),
        ],
    ));
    let code = Arc::new(StreamingFake::new(
        "fake:code",
        vec!["fn ", "main() ", "{}\n"],
    ));
    let math = Arc::new(StreamingFake::new("fake:math", vec!["2+", "2=4"]));

    let cond = Conductor::builder()
        .coordinator(coordinator)
        .worker("code", code)
        .worker("math", math)
        .max_steps(5)
        .verifier(Arc::new(AlwaysScore(0.4)))
        .build()
        .unwrap();

    let mut stream = cond
        .stream(&Principal::anonymous(), OrchInput::from_user("plan"))
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

    // Exactly one final per worker — no partials.
    assert_eq!(scores.len(), 2, "got events: {scores:?}");
    for (step, _, score) in &scores {
        assert_eq!(*step, 0);
        assert!((score - 0.4).abs() < 1e-6, "expected 0.4, got {score}");
    }
    let mut branches: Vec<u32> = scores.iter().map(|(_, b, _)| *b).collect();
    branches.sort();
    assert_eq!(branches, vec![1, 2]);
}

#[tokio::test]
async fn conductor_no_partials_for_non_streaming_workers() {
    // Workers are non-streaming `FakeProvider`s — they fall through to
    // `chat()` and never post a `Delta`. The streaming verifier hook is
    // therefore never invoked even when overridden.
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![coord_dispatch(&[("code", "do code")]), coord_halt("done")],
    ));
    let code = Arc::new(FakeProvider::new(
        "fake:code",
        vec![assistant_text("non-streamed worker output")],
    ));

    let v = Arc::new(CountingStreamingVerifier::default());
    let cond = Conductor::builder()
        .coordinator(coordinator)
        .worker("code", code)
        .max_steps(5)
        .verifier(v.clone())
        .build()
        .unwrap();

    let mut stream = cond
        .stream(&Principal::anonymous(), OrchInput::from_user("hi"))
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

    // No streaming hook calls; one synthesis-complete final.
    assert_eq!(v.streaming_calls.load(Ordering::SeqCst), 0);
    assert_eq!(scores.len(), 1, "got events: {scores:?}");
    assert_eq!(scores[0].0, 0);
    assert_eq!(scores[0].1, 1);
    assert!((scores[0].2 - 0.9).abs() < 1e-6);
}

#[tokio::test]
async fn conductor_branch_index_stable_under_concurrent_completion() {
    // Two streaming workers — `slow` carries a 50ms per-delta delay so
    // it completes after `fast`. Per-worker branch identity must remain
    // stable across all of that worker's partials and its final, even
    // when workers complete out of dispatch order.
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![
            coord_dispatch(&[("fast", "do fast"), ("slow", "do slow")]),
            coord_halt("done"),
        ],
    ));
    let fast = Arc::new(StreamingFake::new("fake:fast", vec!["F1", "F2"]));
    let slow = Arc::new(
        StreamingFake::new("fake:slow", vec!["S1", "S2"])
            .with_per_delta_delay(Duration::from_millis(50)),
    );

    let v = Arc::new(CountingStreamingVerifier::default());
    let cond = Conductor::builder()
        .coordinator(coordinator)
        .worker("fast", fast)
        .worker("slow", slow)
        .max_steps(5)
        .verifier(v.clone())
        .build()
        .unwrap();

    let mut stream = cond
        .stream(&Principal::anonymous(), OrchInput::from_user("plan"))
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

    // 2 partials per worker (2 deltas each) + 1 final per worker.
    assert_eq!(scores.len(), 6, "got events: {scores:?}");

    // Filter by branch and assert each worker's events are correctly
    // labelled. `fast` is dispatched first → branch=1; `slow` is
    // dispatched second → branch=2; identity travels with the worker
    // regardless of completion order.
    let fast_events: Vec<_> = scores.iter().filter(|(_, b, _)| *b == 1).collect();
    let slow_events: Vec<_> = scores.iter().filter(|(_, b, _)| *b == 2).collect();
    assert_eq!(fast_events.len(), 3, "fast events: {fast_events:?}");
    assert_eq!(slow_events.len(), 3, "slow events: {slow_events:?}");

    // Each worker has exactly one synthesis-complete final (score 0.9).
    let fast_finals: Vec<_> = fast_events
        .iter()
        .filter(|(_, _, s)| (s - 0.9).abs() < 1e-6)
        .collect();
    let slow_finals: Vec<_> = slow_events
        .iter()
        .filter(|(_, _, s)| (s - 0.9).abs() < 1e-6)
        .collect();
    assert_eq!(fast_finals.len(), 1);
    assert_eq!(slow_finals.len(), 1);
}

#[tokio::test]
async fn conductor_tool_call_starts_precede_any_verifier_score() {
    // Ordering invariant: all `ToolCallStart` events for a step must be
    // emitted before any `VerifierScore` (partial or final) for that
    // step. Workers spawn after the start-events go out, so this is the
    // load-bearing guarantee that consumers can rely on for tracing.
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![
            coord_dispatch(&[("a", "do a"), ("b", "do b")]),
            coord_halt("done"),
        ],
    ));
    let a = Arc::new(StreamingFake::new("fake:a", vec!["A"]));
    let b = Arc::new(StreamingFake::new("fake:b", vec!["B"]));

    let v = Arc::new(CountingStreamingVerifier::default());
    let cond = Conductor::builder()
        .coordinator(coordinator)
        .worker("a", a)
        .worker("b", b)
        .max_steps(5)
        .verifier(v)
        .build()
        .unwrap();

    let mut stream = cond
        .stream(&Principal::anonymous(), OrchInput::from_user("plan"))
        .await;
    let mut order: Vec<&'static str> = Vec::new();
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            OrchEvent::ToolCallStart { .. } => order.push("start"),
            OrchEvent::VerifierScore { .. } => order.push("score"),
            _ => {}
        }
    }
    // Find the index of the first "score" — every "start" must precede it.
    let first_score = order.iter().position(|&e| e == "score").unwrap();
    let starts_before: usize = order[..first_score]
        .iter()
        .filter(|&&e| e == "start")
        .count();
    assert_eq!(
        starts_before, 2,
        "expected 2 ToolCallStart before any VerifierScore: {order:?}"
    );
}

/// Phase 16.A.2 — bounded mpsc backpressure regression test.
///
/// Drives `Conductor::stream` through far more deltas (256) per worker
/// than the channel capacity (64) under a `CountingStreamingVerifier`
/// so each delta also produces a `VerifierScore` partial. With the
/// Phase 14.A unbounded channel this exercised pure memory growth;
/// with the Phase 16.A.2 bounded channel the spawned worker tasks
/// must repeatedly block on `send().await` until the recv-loop drains.
/// The test passes iff every delta is delivered and the
/// streaming-verifier hook fires exactly N times — i.e. backpressure
/// neither drops events nor deadlocks the worker tasks.
#[tokio::test]
async fn conductor_stream_bounded_backpressure_high_delta_count() {
    const N_DELTAS: usize = 256; // 4× the 64-slot bound
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![coord_dispatch(&[("code", "go")]), coord_halt("done")],
    ));
    let deltas: Vec<&str> = std::iter::repeat_n("x", N_DELTAS).collect();
    let code = Arc::new(StreamingFake::new("fake:code", deltas));

    let v = Arc::new(CountingStreamingVerifier::default());
    let cond = Conductor::builder()
        .coordinator(coordinator)
        .worker("code", code)
        .max_steps(5)
        .verifier(v.clone())
        .build()
        .unwrap();

    let mut stream = cond
        .stream(&Principal::anonymous(), OrchInput::from_user("go"))
        .await;
    let mut partial_scores = 0_usize;
    while let Some(ev) = stream.next().await {
        if let OrchEvent::VerifierScore { score, .. } = ev.unwrap() {
            if (score - 0.5).abs() < 1e-6 {
                partial_scores += 1;
            }
        }
    }

    // Every produced delta crossed the bounded worker channel without
    // loss, and every delta was passed through the verifier.
    assert_eq!(partial_scores, N_DELTAS, "verifier partials dropped");
    assert_eq!(v.streaming_calls.load(Ordering::SeqCst), N_DELTAS);
}
