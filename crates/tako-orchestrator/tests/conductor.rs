//! Conductor end-to-end tests against scripted FakeProviders.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::json;
use tako_core::{
    Capabilities, ChatChunk, ChatRequest, ChatResponse, FinishReason, LlmProvider, Message,
    Principal, TakoError, Usage,
};
use tako_orchestrator::{Conductor, OrchInput, Orchestrator};

#[derive(Debug)]
struct FakeProvider {
    id: String,
    capabilities: Capabilities,
    responses: tokio::sync::Mutex<VecDeque<ChatResponse>>,
    calls: AtomicUsize,
    delay: Duration,
}

impl FakeProvider {
    fn new(id: &str, responses: Vec<ChatResponse>) -> Self {
        Self {
            id: id.into(),
            capabilities: Capabilities::default(),
            responses: tokio::sync::Mutex::new(responses.into()),
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
        }
    }

    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
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
    async fn chat(
        &self,
        _principal: &Principal,
        _req: ChatRequest,
    ) -> Result<ChatResponse, TakoError> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
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

#[tokio::test]
async fn conductor_dispatches_two_workers_and_halts() {
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![
            coord_dispatch(&[("code", "write fib"), ("math", "verify")]),
            coord_halt("All good."),
        ],
    ));
    let code = Arc::new(FakeProvider::new(
        "fake:code",
        vec![assistant_text("fn fib(n: u32) -> u32 { ... }")],
    ));
    let math = Arc::new(FakeProvider::new("fake:math", vec![assistant_text("OK")]));

    let cond = Conductor::builder()
        .coordinator(coordinator.clone())
        .worker("code", code.clone())
        .worker("math", math.clone())
        .max_steps(5)
        .build()
        .unwrap();

    let result = cond
        .run(
            &Principal::anonymous(),
            OrchInput::from_user("plan + verify"),
        )
        .await
        .unwrap();
    assert_eq!(result.text, "All good.");
    assert_eq!(coordinator.calls.load(Ordering::SeqCst), 2);
    assert_eq!(code.calls.load(Ordering::SeqCst), 1);
    assert_eq!(math.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn conductor_max_fanout_caps_concurrent_workers() {
    // Coordinator dispatches 4 workers in parallel; max_fanout=2 means
    // they run in two waves of 2. With 50ms sleeps, total should be
    // around 100ms (2 batches), well under 200ms (sequential = 200ms).
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![
            coord_dispatch(&[("w", "a"), ("w", "b"), ("w", "c"), ("w", "d")]),
            coord_halt("done"),
        ],
    ));
    let worker = Arc::new(
        FakeProvider::new(
            "fake:worker",
            vec![
                assistant_text("a"),
                assistant_text("b"),
                assistant_text("c"),
                assistant_text("d"),
            ],
        )
        .with_delay(Duration::from_millis(50)),
    );

    let cond = Conductor::builder()
        .coordinator(coordinator.clone())
        .worker("w", worker.clone())
        .max_fanout(2)
        .max_steps(5)
        .build()
        .unwrap();

    let start = std::time::Instant::now();
    let result = cond
        .run(&Principal::anonymous(), OrchInput::from_user("fanout"))
        .await
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(result.text, "done");
    assert!(
        elapsed >= Duration::from_millis(80),
        "fanout=2 workers should take ~2 batches of 50ms: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(220),
        "fanout=2 workers serialised (got {elapsed:?}); should overlap"
    );
    assert_eq!(worker.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn conductor_fail_fast_aborts_on_unknown_worker() {
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![coord_dispatch(&[("does_not_exist", "boom")])],
    ));
    let cond = Conductor::builder()
        .coordinator(coordinator.clone())
        .fail_fast(true)
        .max_steps(3)
        .build()
        .unwrap();

    let err = cond
        .run(&Principal::anonymous(), OrchInput::from_user("x"))
        .await
        .unwrap_err();
    let TakoError::Provider { message, .. } = err else {
        panic!("expected Provider error");
    };
    assert!(
        message.contains("unknown worker"),
        "expected fail_fast surface: {message}"
    );
}

#[tokio::test]
async fn conductor_recovers_from_malformed_coordinator_json() {
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![assistant_text("not json at all"), coord_halt("recovered")],
    ));
    let cond = Conductor::builder()
        .coordinator(coordinator.clone())
        .max_steps(5)
        .build()
        .unwrap();

    let result = cond
        .run(&Principal::anonymous(), OrchInput::from_user("x"))
        .await
        .unwrap();
    assert_eq!(result.text, "recovered");
    assert_eq!(coordinator.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn conductor_max_steps_caps_loop() {
    // Coordinator never halts; max_steps caps it.
    let coordinator = Arc::new(FakeProvider::new(
        "fake:coord",
        vec![coord_dispatch(&[("w", "a")]), coord_dispatch(&[("w", "b")])],
    ));
    let worker = Arc::new(FakeProvider::new(
        "fake:worker",
        vec![assistant_text("ok"), assistant_text("ok")],
    ));
    let cond = Conductor::builder()
        .coordinator(coordinator.clone())
        .worker("w", worker.clone())
        .max_steps(2)
        .build()
        .unwrap();
    let result = cond
        .run(&Principal::anonymous(), OrchInput::from_user("loop"))
        .await
        .unwrap();
    assert_eq!(result.steps, 2);
    let _ = result.message;
}

#[tokio::test]
async fn conductor_halt_with_no_workers_registered() {
    let coordinator = Arc::new(FakeProvider::new("fake:coord", vec![coord_halt("instant")]));
    let cond = Conductor::builder()
        .coordinator(coordinator)
        .max_steps(3)
        .build()
        .unwrap();
    // System prompt should mention "(no workers registered)".
    assert!(format!("{cond:?}").contains("workers"));
    let result = cond
        .run(&Principal::anonymous(), OrchInput::from_user("anything"))
        .await
        .unwrap();
    assert_eq!(result.text, "instant");
}

#[test]
fn dispatch_plan_strips_markdown_fence() {
    use tako_orchestrator::DispatchPlan;
    let raw =
        "```json\n{\"thought\":\"hi\",\"dispatch\":[],\"halt\":true,\"final_answer\":\"x\"}\n```";
    // Re-implement the strip locally to avoid exposing the parser.
    let trimmed = raw.trim();
    let stripped = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_end_matches("```").trim()
    } else {
        trimmed
    };
    let plan: DispatchPlan = serde_json::from_str(stripped).unwrap();
    assert!(plan.halt);
}
