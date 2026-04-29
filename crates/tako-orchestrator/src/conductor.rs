//! `Conductor` orchestrator: a coordinator LLM dispatches tasks to a
//! pool of worker providers in parallel.
//!
//! Generalisation of arXiv:2512.04388 (Sakana AI's *Conductor*). The
//! coordinator emits structured natural-language dispatch instructions
//! (validated as JSON against [`DispatchPlan`]); each worker is keyed by
//! a role name (e.g. `"code"`, `"math"`). Workers run concurrently
//! through an `Arc<Semaphore>` capped at `max_fanout`; each is
//! independently bounded by `worker_timeout`. `fail_fast` aborts on the
//! first worker error; otherwise partial results are folded into the
//! next coordinator turn.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tako_core::{
    ChatRequest, ContentPart, FinishReason, LlmProvider, Message, Principal, Role, TakoError, Usage,
};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{Instrument, info_span};

use crate::types::{OrchEvent, OrchInput, OrchOutput};
use crate::{Orchestrator, OrchestratorKind};

const DEFAULT_MAX_STEPS: u32 = 6;
const DEFAULT_MAX_FANOUT: usize = 4;
const DEFAULT_WORKER_TIMEOUT: Duration = Duration::from_secs(120);

/// One worker dispatch the coordinator wants to issue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerDispatch {
    /// Worker pool key. Must match a name registered with the [`Conductor`].
    pub worker: String,
    /// Free-form natural-language task for the worker.
    pub task: String,
}

/// Structured dispatch plan emitted by the coordinator at each turn.
///
/// The coordinator is given this schema in its system prompt and must
/// return JSON conforming to it. Malformed output is fed back as a
/// retry prompt for one extra turn before the orchestrator gives up.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DispatchPlan {
    /// Optional internal monologue. The orchestrator passes this through
    /// for OTel spans but doesn't act on it.
    #[serde(default)]
    pub thought: Option<String>,
    /// Workers to dispatch in parallel for this turn. Empty means
    /// "no workers needed; just decide whether to halt".
    #[serde(default)]
    pub dispatch: Vec<WorkerDispatch>,
    /// When `true`, [`final_answer`](Self::final_answer) is the user
    /// response; the loop terminates.
    #[serde(default)]
    pub halt: bool,
    /// Required iff `halt == true`.
    #[serde(default)]
    pub final_answer: Option<String>,
}

/// Builder + struct for the Conductor orchestrator.
pub struct Conductor {
    coordinator: Arc<dyn LlmProvider>,
    workers: HashMap<String, Arc<dyn LlmProvider>>,
    max_steps: u32,
    max_fanout: usize,
    worker_timeout: Duration,
    fail_fast: bool,
}

impl std::fmt::Debug for Conductor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conductor")
            .field("coordinator", &self.coordinator.id())
            .field("workers", &self.workers.keys().collect::<Vec<_>>())
            .field("max_steps", &self.max_steps)
            .field("max_fanout", &self.max_fanout)
            .field("worker_timeout", &self.worker_timeout)
            .field("fail_fast", &self.fail_fast)
            .finish()
    }
}

impl Conductor {
    pub fn builder() -> ConductorBuilder {
        ConductorBuilder::default()
    }

    /// System prompt fed to the coordinator at every turn so it knows the
    /// dispatch JSON schema and the available worker roles.
    fn system_prompt(&self) -> String {
        let mut roles: Vec<String> = self
            .workers
            .iter()
            .map(|(k, v)| format!("  - `{k}` (provider: `{}`)", v.id()))
            .collect();
        roles.sort();
        let roles = if roles.is_empty() {
            "  (no workers registered)".to_string()
        } else {
            roles.join("\n")
        };
        format!(
            "You are the COORDINATOR in a multi-agent system. At each turn you MUST emit \
             a single JSON object (no prose, no markdown) matching this schema:\n\n\
             {{\n  \"thought\": \"<internal monologue>\",\n  \"dispatch\": [\n    \
             {{\"worker\": \"<role>\", \"task\": \"<task>\"}}, ...\n  ],\n  \
             \"halt\": <bool>,\n  \"final_answer\": \"<answer when halt is true>\"\n}}\n\n\
             Available worker roles:\n{roles}\n\n\
             Set `halt: true` and provide `final_answer` only when the user's request is fully \
             addressed. Otherwise dispatch one or more workers in parallel; their results will \
             come back as a single user-role message before your next turn."
        )
    }
}

#[derive(Default)]
pub struct ConductorBuilder {
    coordinator: Option<Arc<dyn LlmProvider>>,
    workers: HashMap<String, Arc<dyn LlmProvider>>,
    max_steps: Option<u32>,
    max_fanout: Option<usize>,
    worker_timeout: Option<Duration>,
    fail_fast: Option<bool>,
}

impl std::fmt::Debug for ConductorBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConductorBuilder")
            .field("coordinator", &self.coordinator.as_ref().map(|c| c.id()))
            .field("workers", &self.workers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl ConductorBuilder {
    pub fn coordinator(mut self, p: Arc<dyn LlmProvider>) -> Self {
        self.coordinator = Some(p);
        self
    }

    pub fn worker(mut self, name: impl Into<String>, p: Arc<dyn LlmProvider>) -> Self {
        self.workers.insert(name.into(), p);
        self
    }

    pub fn max_steps(mut self, n: u32) -> Self {
        self.max_steps = Some(n.max(1));
        self
    }

    pub fn max_fanout(mut self, n: usize) -> Self {
        self.max_fanout = Some(n.max(1));
        self
    }

    pub fn worker_timeout(mut self, d: Duration) -> Self {
        self.worker_timeout = Some(d);
        self
    }

    pub fn fail_fast(mut self, b: bool) -> Self {
        self.fail_fast = Some(b);
        self
    }

    pub fn build(self) -> Result<Conductor, TakoError> {
        Ok(Conductor {
            coordinator: self.coordinator.ok_or_else(|| {
                TakoError::Invalid("ConductorBuilder: coordinator is required".into())
            })?,
            workers: self.workers,
            max_steps: self.max_steps.unwrap_or(DEFAULT_MAX_STEPS),
            max_fanout: self.max_fanout.unwrap_or(DEFAULT_MAX_FANOUT),
            worker_timeout: self.worker_timeout.unwrap_or(DEFAULT_WORKER_TIMEOUT),
            fail_fast: self.fail_fast.unwrap_or(false),
        })
    }
}

/// Result of running one worker in a fanout.
#[derive(Clone, Debug)]
struct WorkerResult {
    name: String,
    task: String,
    outcome: Result<String, String>,
}

#[async_trait]
impl Orchestrator for Conductor {
    fn kind(&self) -> OrchestratorKind {
        OrchestratorKind::Conductor
    }

    async fn run(&self, principal: &Principal, input: OrchInput) -> Result<OrchOutput, TakoError> {
        let span = info_span!(
            "tako.orchestrator.run",
            "tako.orchestrator.kind" = "conductor",
            "tako.principal.tenant_id" = %principal.tenant_id,
            "tako.principal.user_id" = %principal.user_id,
        );
        async move {
            let mut messages: Vec<Message> = Vec::new();
            // The coordinator system prompt is constant; user-provided
            // system text is appended after it.
            messages.push(Message::system(self.system_prompt()));
            if let Some(extra) = input.system.clone() {
                messages.push(Message::system(extra));
            }
            messages.extend(input.messages);

            let coord_id = self.coordinator.id().to_string();
            let model = coord_id.split(':').nth(1).unwrap_or("").to_string();

            let mut total_usage = Usage::default();
            let mut steps = 0_u32;
            let semaphore = Arc::new(Semaphore::new(self.max_fanout));

            for step in 0..self.max_steps {
                let req = ChatRequest::new(model.clone(), messages.clone());
                let resp = {
                    let span = info_span!(
                        "tako.provider.chat",
                        "tako.provider.id" = %coord_id,
                        "tako.orchestrator.step" = step,
                        "tako.orchestrator.role" = "coordinator",
                    );
                    self.coordinator
                        .chat(principal, req)
                        .instrument(span)
                        .await?
                };
                steps += 1;
                total_usage.input_tokens = total_usage
                    .input_tokens
                    .saturating_add(resp.usage.input_tokens);
                total_usage.output_tokens = total_usage
                    .output_tokens
                    .saturating_add(resp.usage.output_tokens);

                let raw_text = resp
                    .message
                    .content
                    .iter()
                    .filter_map(ContentPart::as_text)
                    .collect::<Vec<_>>()
                    .join("");

                messages.push(resp.message.clone());

                let plan = match parse_dispatch_plan(&raw_text) {
                    Ok(p) => p,
                    Err(e) => {
                        // Feed the parse error back so the coordinator can correct.
                        let retry = format!(
                            "Your previous output did not match the dispatch JSON schema: {e}. \
                             Reply with a single valid JSON object."
                        );
                        messages.push(Message::user(retry));
                        continue;
                    }
                };

                if plan.halt {
                    let final_text = plan.final_answer.unwrap_or_else(|| raw_text.clone());
                    return Ok(OrchOutput {
                        text: final_text.clone(),
                        message: Message::assistant(final_text),
                        usage: total_usage,
                        steps,
                    });
                }

                if plan.dispatch.is_empty() {
                    // Nothing to do this turn but the coordinator didn't halt either.
                    // Nudge it toward a final answer.
                    messages.push(Message::user(
                        "You returned an empty dispatch with halt=false. \
                         Either dispatch a worker or halt with a final answer.",
                    ));
                    continue;
                }

                let results = self
                    .dispatch_workers(principal, plan.dispatch, Arc::clone(&semaphore), step)
                    .await?;

                if self.fail_fast {
                    if let Some(err) = results.iter().find_map(|r| r.outcome.as_ref().err()) {
                        return Err(TakoError::Provider {
                            message: format!("Conductor worker failed (fail_fast): {err}"),
                            source: None,
                            details: Box::new(tako_core::ProviderErrorDetails {
                                provider_id: coord_id.clone(),
                                model: model.clone(),
                                ..Default::default()
                            }),
                        });
                    }
                }

                let summary = render_worker_results(&results);
                messages.push(Message::user(summary));
            }

            // Hit max_steps without an explicit halt. Return whatever the
            // coordinator's last assistant message said.
            let last_text = messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, Role::Assistant))
                .and_then(|m| {
                    m.content
                        .iter()
                        .find_map(ContentPart::as_text)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            Ok(OrchOutput {
                text: last_text.clone(),
                message: Message::assistant(last_text),
                usage: total_usage,
                steps,
            })
        }
        .instrument(span)
        .await
    }

    async fn stream(
        &self,
        principal: &Principal,
        input: OrchInput,
    ) -> BoxStream<'static, Result<OrchEvent, TakoError>> {
        let coordinator = self.coordinator.clone();
        let workers = self.workers.clone();
        let max_steps = self.max_steps;
        let max_fanout = self.max_fanout;
        let worker_timeout = self.worker_timeout;
        let fail_fast = self.fail_fast;
        let principal = principal.clone();
        let system = self.system_prompt();

        let s = async_stream::try_stream! {
            let mut messages: Vec<Message> = Vec::new();
            messages.push(Message::system(system));
            if let Some(extra) = input.system.clone() {
                messages.push(Message::system(extra));
            }
            messages.extend(input.messages);

            let coord_id = coordinator.id().to_string();
            let model = coord_id.split(':').nth(1).unwrap_or("").to_string();
            let mut total_usage = Usage::default();
            let mut steps = 0_u32;
            let semaphore = Arc::new(Semaphore::new(max_fanout));

            for step in 0..max_steps {
                yield OrchEvent::StepStart { step };

                let req = ChatRequest::new(model.clone(), messages.clone());
                let span = info_span!(
                    "tako.provider.chat",
                    "tako.provider.id" = %coord_id,
                    "tako.orchestrator.step" = step,
                    "tako.orchestrator.role" = "coordinator",
                );
                let resp = coordinator
                    .chat(&principal, req)
                    .instrument(span)
                    .await?;
                steps += 1;
                total_usage.input_tokens = total_usage
                    .input_tokens
                    .saturating_add(resp.usage.input_tokens);
                total_usage.output_tokens = total_usage
                    .output_tokens
                    .saturating_add(resp.usage.output_tokens);

                let raw_text = resp
                    .message
                    .content
                    .iter()
                    .filter_map(ContentPart::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                if !raw_text.is_empty() {
                    yield OrchEvent::AssistantText {
                        step,
                        delta: raw_text.clone(),
                    };
                }

                messages.push(resp.message.clone());

                let plan = match parse_dispatch_plan(&raw_text) {
                    Ok(p) => p,
                    Err(e) => {
                        let retry = format!(
                            "Your previous output did not match the dispatch JSON schema: {e}. \
                             Reply with a single valid JSON object."
                        );
                        messages.push(Message::user(retry));
                        continue;
                    }
                };

                if plan.halt {
                    let final_text = plan.final_answer.unwrap_or_else(|| raw_text.clone());
                    yield OrchEvent::Final {
                        output: Box::new(OrchOutput {
                            text: final_text.clone(),
                            message: Message::assistant(final_text),
                            usage: total_usage,
                            steps,
                        }),
                    };
                    return;
                }

                if plan.dispatch.is_empty() {
                    messages.push(Message::user(
                        "You returned an empty dispatch with halt=false. \
                         Either dispatch a worker or halt with a final answer.",
                    ));
                    continue;
                }

                // Emit tool-call-start events for each dispatched worker so
                // SSE consumers can see the fanout shape (workers are surfaced
                // as `worker:<role>` tool calls — there is no first-class
                // worker event in OrchEvent).
                for (idx, d) in plan.dispatch.iter().enumerate() {
                    yield OrchEvent::ToolCallStart {
                        step,
                        name: format!("worker:{}", d.worker),
                        id: format!("step{step}-{idx}"),
                    };
                }

                let results = dispatch_workers_static(
                    &workers,
                    &principal,
                    plan.dispatch.clone(),
                    Arc::clone(&semaphore),
                    step,
                    worker_timeout,
                )
                .await;

                for (idx, r) in results.iter().enumerate() {
                    let id = format!("step{step}-{idx}");
                    let (value, is_err) = match &r.outcome {
                        Ok(text) => (
                            serde_json::json!({ "worker": r.name, "result": text }),
                            false,
                        ),
                        Err(e) => (
                            serde_json::json!({ "worker": r.name, "error": e }),
                            true,
                        ),
                    };
                    yield OrchEvent::ToolCallResult {
                        step,
                        id,
                        result: value,
                        is_error: is_err,
                    };
                }

                if fail_fast {
                    if let Some(err) = results.iter().find_map(|r| r.outcome.as_ref().err()) {
                        Err(TakoError::Provider {
                            message: format!("Conductor worker failed (fail_fast): {err}"),
                            source: None,
                            details: Box::new(tako_core::ProviderErrorDetails {
                                provider_id: coord_id.clone(),
                                model: model.clone(),
                                ..Default::default()
                            }),
                        })?;
                    }
                }

                let summary = render_worker_results(&results);
                messages.push(Message::user(summary));
            }

            // Hit max_steps without explicit halt.
            let last_text = messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, Role::Assistant))
                .and_then(|m| {
                    m.content
                        .iter()
                        .find_map(ContentPart::as_text)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            yield OrchEvent::Final {
                output: Box::new(OrchOutput {
                    text: last_text.clone(),
                    message: Message::assistant(last_text),
                    usage: total_usage,
                    steps,
                }),
            };
        };

        Box::pin(s)
    }
}

impl Conductor {
    async fn dispatch_workers(
        &self,
        principal: &Principal,
        plan: Vec<WorkerDispatch>,
        sem: Arc<Semaphore>,
        step: u32,
    ) -> Result<Vec<WorkerResult>, TakoError> {
        Ok(dispatch_workers_static(
            &self.workers,
            principal,
            plan,
            sem,
            step,
            self.worker_timeout,
        )
        .await)
    }
}

/// Free-function variant of `Conductor::dispatch_workers` that captures only
/// owned data — needed inside the `async_stream::try_stream!` closure where
/// `&self` is unavailable across the yield points.
async fn dispatch_workers_static(
    workers: &HashMap<String, Arc<dyn LlmProvider>>,
    principal: &Principal,
    plan: Vec<WorkerDispatch>,
    sem: Arc<Semaphore>,
    step: u32,
    timeout_dur: Duration,
) -> Vec<WorkerResult> {
    let mut handles = Vec::with_capacity(plan.len());
    for d in plan {
        let Some(provider) = workers.get(&d.worker).cloned() else {
            handles.push(tokio::spawn(async move {
                WorkerResult {
                    name: d.worker.clone(),
                    task: d.task,
                    outcome: Err(format!("unknown worker `{}`", d.worker)),
                }
            }));
            continue;
        };
        let principal = principal.clone();
        let sem = Arc::clone(&sem);
        let span = info_span!(
            "tako.orchestrator.dispatch",
            "tako.orchestrator.step" = step,
            "tako.worker.name" = %d.worker,
            "tako.worker.provider.id" = %provider.id(),
        );
        let task_text = d.task.clone();
        let worker_name = d.worker.clone();
        handles.push(tokio::spawn(
            async move {
                let _permit = match sem.acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => {
                        return WorkerResult {
                            name: worker_name,
                            task: task_text,
                            outcome: Err("semaphore closed".into()),
                        };
                    }
                };
                let model = provider.id().split(':').nth(1).unwrap_or("").to_string();
                let req = ChatRequest::new(model, vec![Message::user(&task_text)]);
                let outcome = match timeout(timeout_dur, provider.chat(&principal, req)).await {
                    Ok(Ok(resp)) => match resp.finish_reason {
                        FinishReason::Stop | FinishReason::ToolCalls => Ok(resp
                            .message
                            .content
                            .iter()
                            .filter_map(ContentPart::as_text)
                            .collect::<Vec<_>>()
                            .join("")),
                        other => Err(format!("worker finished with unexpected reason: {other:?}")),
                    },
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_) => Err(format!("worker timed out after {:?}", timeout_dur)),
                };
                WorkerResult {
                    name: worker_name,
                    task: task_text,
                    outcome,
                }
            }
            .instrument(span),
        ));
    }

    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        match h.await {
            Ok(r) => results.push(r),
            Err(e) => results.push(WorkerResult {
                name: "<join>".into(),
                task: String::new(),
                outcome: Err(format!("join error: {e}")),
            }),
        }
    }
    results
}

fn parse_dispatch_plan(raw: &str) -> Result<DispatchPlan, String> {
    // The coordinator may wrap its JSON in markdown fences; strip the
    // outermost fence pair if present.
    let trimmed = raw.trim();
    let stripped = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_end_matches("```").trim()
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_end_matches("```").trim()
    } else {
        trimmed
    };
    serde_json::from_str::<DispatchPlan>(stripped).map_err(|e| e.to_string())
}

fn render_worker_results(results: &[WorkerResult]) -> String {
    let mut out = String::from("Worker results:\n");
    for r in results {
        match &r.outcome {
            Ok(text) => out.push_str(&format!(
                "- {} (task: {:?}): {}\n",
                r.name,
                short(&r.task),
                text
            )),
            Err(err) => out.push_str(&format!(
                "- {} (task: {:?}): ERROR: {}\n",
                r.name,
                short(&r.task),
                err
            )),
        }
    }
    out
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.len() <= 60 {
        s.to_string()
    } else {
        format!("{}...", &s[..57])
    }
}
