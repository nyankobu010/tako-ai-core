//! Trinity orchestrator end-to-end tests against scripted FakeProviders.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, FinishReason, LlmProvider, Message,
    Principal, TakoError, Usage,
};
use tako_orchestrator::{OrchEvent, OrchInput, Orchestrator, RegexRouter, Trinity};

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

/// Streaming-capable fake. Emits a fixed series of text deltas, then End.
#[derive(Debug)]
struct StreamingFake {
    id: String,
    capabilities: Capabilities,
    deltas: Vec<String>,
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
        Err(TakoError::Invalid("StreamingFake.chat not used".into()))
    }
    async fn stream(
        &self,
        _p: &Principal,
        _r: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, TakoError>>, TakoError> {
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

#[tokio::test]
async fn trinity_stream_forwards_deltas() {
    let code = Arc::new(StreamingFake::new(
        "fake:code",
        vec!["fn ", "main() ", "{}\n"],
    ));
    let math = Arc::new(FakeProvider::new("fake:math", vec![assistant("X")]));
    let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("X")]));
    let trinity = Trinity::builder()
        .role("code", code.clone())
        .role("math", math.clone())
        .role("fallback", fb.clone())
        .router(Arc::new(RegexRouter::default()))
        .max_steps(2)
        .build()
        .unwrap();

    let mut stream = trinity
        .stream(
            &Principal::anonymous(),
            OrchInput::from_user("Write a fn to compute fib"),
        )
        .await;

    let mut text_deltas: Vec<String> = Vec::new();
    let mut saw_step_start = false;
    let mut saw_final = false;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            OrchEvent::StepStart { .. } => {
                saw_step_start = true;
            }
            OrchEvent::AssistantText { delta, .. } => {
                text_deltas.push(delta);
            }
            OrchEvent::Final { output } => {
                saw_final = true;
                assert_eq!(output.text, "fn main() {}\n");
            }
            _ => {}
        }
    }
    assert!(saw_step_start, "expected at least one StepStart event");
    assert!(saw_final, "expected a Final event");
    assert_eq!(text_deltas, vec!["fn ", "main() ", "{}\n"]);
}

#[tokio::test]
async fn trinity_stream_falls_back_when_no_streaming() {
    // FakeProvider has supports_streaming=false; Trinity must fall back
    // to chat() and emit one synthetic AssistantText.
    let code = Arc::new(FakeProvider::new(
        "fake:code",
        vec![assistant("non-streamed")],
    ));
    let math = Arc::new(FakeProvider::new("fake:math", vec![assistant("X")]));
    let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("X")]));
    let trinity = Trinity::builder()
        .role("code", code.clone())
        .role("math", math.clone())
        .role("fallback", fb.clone())
        .router(Arc::new(RegexRouter::default()))
        .max_steps(2)
        .build()
        .unwrap();

    let mut stream = trinity
        .stream(
            &Principal::anonymous(),
            OrchInput::from_user("Write a fn to compute fib"),
        )
        .await;

    let mut text_deltas: Vec<String> = Vec::new();
    let mut saw_final = false;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            OrchEvent::AssistantText { delta, .. } => {
                text_deltas.push(delta);
            }
            OrchEvent::Final { output } => {
                saw_final = true;
                assert_eq!(output.text, "non-streamed");
            }
            _ => {}
        }
    }
    assert!(saw_final);
    assert_eq!(text_deltas, vec!["non-streamed"]);
    assert_eq!(code.calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// Phase 6.B — Budget wiring.
// ---------------------------------------------------------------------------

fn assistant_with_usage(text: &str, input: u32, output: u32) -> ChatResponse {
    ChatResponse {
        message: Message::assistant(text),
        finish_reason: FinishReason::Stop,
        usage: Usage {
            input_tokens: input,
            output_tokens: output,
        },
        raw: Default::default(),
    }
}

#[tokio::test]
async fn trinity_budget_records_usage_after_chat() {
    use std::collections::BTreeMap;
    use tako_core::Budget;
    use tako_runtime::{BudgetBackend, BudgetTracker, InMemoryBudgetBackend};

    let code = Arc::new(FakeProvider::new(
        "fake:code",
        vec![assistant_with_usage("CODE OUT", 9, 5)],
    ));
    let backend = Arc::new(InMemoryBudgetBackend::new());
    let tracker = Arc::new(BudgetTracker::new(
        Arc::clone(&backend) as Arc<dyn BudgetBackend>,
        Budget::default(),
    ));

    let trinity = Trinity::builder()
        .role("code", code.clone())
        .role("fallback", code.clone())
        .router(Arc::new(RegexRouter::default()))
        .max_steps(1)
        .budget(Arc::clone(&tracker))
        .build()
        .unwrap();

    let principal = Principal {
        tenant_id: "tenant-trinity".into(),
        user_id: "u".into(),
        roles: vec![],
        trace_id: None,
        metadata: BTreeMap::new(),
    };
    trinity
        .run(
            &principal,
            OrchInput::from_user("Write a fn to compute fib"),
        )
        .await
        .unwrap();

    let usage = backend.current_usage("tenant-trinity").await.unwrap();
    assert_eq!(usage.tokens_today, 14);
}

#[tokio::test]
async fn trinity_budget_pre_check_short_circuits_on_daily_cap() {
    use std::collections::BTreeMap;
    use tako_core::Budget;
    use tako_runtime::{BudgetBackend, BudgetTracker, InMemoryBudgetBackend};

    let backend = Arc::new(InMemoryBudgetBackend::new());
    backend.record("tenant-y", 5.0, 0).await.unwrap();
    let tracker = Arc::new(BudgetTracker::new(
        Arc::clone(&backend) as Arc<dyn BudgetBackend>,
        Budget {
            max_usd_per_request: None,
            max_tokens_per_request: None,
            max_usd_per_day: Some(1.0),
            max_usd_per_tenant_per_day: BTreeMap::new(),
        },
    ));
    let code = Arc::new(FakeProvider::new(
        "fake:code",
        vec![assistant("never-called")],
    ));
    let trinity = Trinity::builder()
        .role("code", code.clone())
        .role("fallback", code.clone())
        .router(Arc::new(RegexRouter::default()))
        .max_steps(2)
        .budget(Arc::clone(&tracker))
        .build()
        .unwrap();

    let principal = Principal {
        tenant_id: "tenant-y".into(),
        user_id: "u".into(),
        roles: vec![],
        trace_id: None,
        metadata: BTreeMap::new(),
    };
    let err = trinity
        .run(&principal, OrchInput::from_user("Write code"))
        .await
        .unwrap_err();
    assert!(matches!(err, TakoError::BudgetExhausted(_)));
    assert_eq!(code.calls.load(Ordering::SeqCst), 0);
}

mod verifier_emits {
    //! Phase 10.C — `Trinity` emits one `OrchEvent::VerifierScore`
    //! per role's assistant turn after the turn completes, with
    //! `branch` = the role's positional index in `role_order`.
    //! Without `.verifier(...)`, no `VerifierScore` events appear
    //! (v0.10.0 byte-for-byte parity).

    use super::{FakeProvider, RegexRouter, Trinity, assistant};
    use futures::StreamExt;
    use std::sync::Arc;
    use tako_core::{AlwaysScore, Principal};
    use tako_orchestrator::{OrchEvent, OrchInput, Orchestrator};

    #[tokio::test]
    async fn trinity_emits_verifier_score_when_attached() {
        let code = Arc::new(FakeProvider::new("fake:code", vec![assistant("CODE OUT")]));
        let math = Arc::new(FakeProvider::new("fake:math", vec![assistant("MATH OUT")]));
        let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("FB")]));
        let trinity = Trinity::builder()
            .role("code", code)
            .role("math", math)
            .role("fallback", fb)
            .router(Arc::new(RegexRouter::default()))
            .max_steps(2)
            .verifier(Arc::new(AlwaysScore(0.6)))
            .build()
            .unwrap();

        let mut stream = trinity
            .stream(
                &Principal::anonymous(),
                OrchInput::from_user("Write a fn to compute fib"),
            )
            .await;
        let mut scores = Vec::new();
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
        // The "code" prompt routes to the `code` role on its first
        // (and only halt-bearing) turn; that role is at positional
        // index 0 in role_order.
        assert_eq!(scores.len(), 1, "expected exactly one VerifierScore");
        assert_eq!(scores[0].0, 0); // step
        assert_eq!(scores[0].1, 0); // branch == role index for `code`
        assert!(
            (scores[0].2 - 0.6).abs() < 1e-6,
            "score should match the AlwaysScore fixture: got {}",
            scores[0].2
        );
    }

    #[tokio::test]
    async fn trinity_emits_no_verifier_score_when_unattached() {
        // Backwards-compat: without `.verifier(...)`, the streaming
        // path emits zero VerifierScore events.
        let code = Arc::new(FakeProvider::new("fake:code", vec![assistant("CODE OUT")]));
        let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("FB")]));
        let trinity = Trinity::builder()
            .role("code", code)
            .role("fallback", fb)
            .router(Arc::new(RegexRouter::default()))
            .max_steps(2)
            .build()
            .unwrap();

        let mut stream = trinity
            .stream(
                &Principal::anonymous(),
                OrchInput::from_user("Write a fn to compute fib"),
            )
            .await;
        let mut count = 0_usize;
        while let Some(ev) = stream.next().await {
            if matches!(ev.unwrap(), OrchEvent::VerifierScore { .. }) {
                count += 1;
            }
        }
        assert_eq!(count, 0);
    }
}

mod streaming_verifier_emits {
    //! Phase 13.B — `Trinity` calls `Verifier::evaluate_streaming`
    //! per assistant-text delta on the *cumulative* buffer when a
    //! verifier is attached. `Ok(Some(score))` returns from the hook
    //! produce intermediate `OrchEvent::VerifierScore` events on the
    //! same `(step, branch)` as the eventual synthesis-complete
    //! final. The default `Ok(None)` impl preserves Phase 10.C
    //! behaviour byte-for-byte.
    use super::{FakeProvider, RegexRouter, StreamingFake, Trinity, assistant};
    use async_trait::async_trait;
    use futures::StreamExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tako_core::{AlwaysScore, Principal, TakoError, Verifier};
    use tako_orchestrator::{OrchEvent, OrchInput, Orchestrator};

    /// Counts every `evaluate_streaming` invocation and returns
    /// `Ok(Some(0.5))` so each call produces a streaming partial.
    /// `score()` (the synthesis-complete final) returns 0.9.
    #[derive(Default)]
    struct CountingStreamingVerifier {
        streaming_calls: AtomicUsize,
        last_partial_len: AtomicUsize,
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
            partial: &str,
        ) -> Result<Option<f32>, TakoError> {
            self.streaming_calls.fetch_add(1, Ordering::SeqCst);
            self.last_partial_len
                .store(partial.len(), Ordering::SeqCst);
            Ok(Some(0.5))
        }
    }

    /// The cumulative buffer must grow monotonically across deltas
    /// — the streaming hook sees an ever-longer prefix, never a
    /// per-delta slice.
    #[tokio::test]
    async fn trinity_emits_per_delta_streaming_verifier_scores() {
        // StreamingFake yields three deltas. We expect:
        //   - 3 streaming-partial VerifierScore events (score 0.5)
        //   - 1 synthesis-complete VerifierScore event (score 0.9)
        // all on the same (step=0, branch=0) for the `code` role.
        let code = Arc::new(StreamingFake::new(
            "fake:code",
            vec!["fn ", "main() ", "{}\n"],
        ));
        let math = Arc::new(FakeProvider::new("fake:math", vec![assistant("X")]));
        let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("X")]));

        let v = Arc::new(CountingStreamingVerifier::default());
        let trinity = Trinity::builder()
            .role("code", code)
            .role("math", math)
            .role("fallback", fb)
            .router(Arc::new(RegexRouter::default()))
            .max_steps(1)
            .verifier(v.clone())
            .build()
            .unwrap();

        let mut stream = trinity
            .stream(
                &Principal::anonymous(),
                OrchInput::from_user("Write a fn to compute fib"),
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

        // Three streaming partials + one synthesis-complete final = four total.
        assert_eq!(scores.len(), 4, "got events: {scores:?}");
        // The first three are partials (score 0.5).
        for (step, branch, score) in &scores[..3] {
            assert_eq!(*step, 0);
            assert_eq!(*branch, 0);
            assert!(
                (*score - 0.5).abs() < 1e-6,
                "expected partial score 0.5, got {score}"
            );
        }
        // The fourth is the synthesis-complete final (score 0.9).
        let (final_step, final_branch, final_score) = scores[3];
        assert_eq!(final_step, 0);
        assert_eq!(final_branch, 0);
        assert!(
            (final_score - 0.9).abs() < 1e-6,
            "expected synthesis final 0.9, got {final_score}"
        );

        // The hook saw exactly one call per delta.
        assert_eq!(v.streaming_calls.load(Ordering::SeqCst), 3);
        // Cumulative buffer length on the last call equals
        // `"fn " + "main() " + "{}\n"`.
        assert_eq!(
            v.last_partial_len.load(Ordering::SeqCst),
            "fn main() {}\n".len()
        );
    }

    /// Regression: a `Verifier` that does not override
    /// `evaluate_streaming` (default `Ok(None)`) produces exactly the
    /// existing single synthesis-complete event — byte-for-byte parity
    /// with Phase 10.C.
    #[tokio::test]
    async fn trinity_default_verifier_emits_only_final_score() {
        let code = Arc::new(StreamingFake::new(
            "fake:code",
            vec!["fn ", "main() ", "{}\n"],
        ));
        let math = Arc::new(FakeProvider::new("fake:math", vec![assistant("X")]));
        let fb = Arc::new(FakeProvider::new("fake:fb", vec![assistant("X")]));

        let trinity = Trinity::builder()
            .role("code", code)
            .role("math", math)
            .role("fallback", fb)
            .router(Arc::new(RegexRouter::default()))
            .max_steps(1)
            // `AlwaysScore` does NOT override `evaluate_streaming`.
            .verifier(Arc::new(AlwaysScore(0.7)))
            .build()
            .unwrap();

        let mut stream = trinity
            .stream(
                &Principal::anonymous(),
                OrchInput::from_user("Write a fn to compute fib"),
            )
            .await;
        let mut scores = Vec::new();
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
        assert_eq!(
            scores.len(),
            1,
            "default-impl Verifier must emit only the synthesis-complete final"
        );
        assert_eq!(scores[0].0, 0);
        assert_eq!(scores[0].1, 0);
        assert!((scores[0].2 - 0.7).abs() < 1e-6);
    }
}
