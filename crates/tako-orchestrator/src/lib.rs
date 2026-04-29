//! `tako-orchestrator` — orchestration strategies for tako.
//!
//! Phase 1 ships [`SingleAgent`]; Phase 2 adds [`Conductor`]. Trinity /
//! AbMcts / SelfCaller arrive in later phases.

pub mod conductor;
pub mod features;
pub mod router;
pub mod self_caller;
pub mod single;
pub mod trinity;
pub mod types;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tako_core::{Principal, TakoError};

pub use conductor::{Conductor, ConductorBuilder, DispatchPlan, WorkerDispatch};
pub use features::{FEATURE_DIM, featurise, featurise_text};
#[cfg(feature = "onnx")]
pub use router::OnnxRouter;
pub use router::{RegexRouter, RegexRouterBuilder};
pub use self_caller::{
    ConfidenceGuard, LlmJudgeGuard, RuleBasedGuard, SelfCaller, SelfCallerBuilder,
};
pub use single::{ChatDefaults, SingleAgent, SingleAgentBuilder};
pub use trinity::{Trinity, TrinityBuilder};
pub use types::{OrchEvent, OrchInput, OrchOutput};

/// What kind of orchestrator. Useful for OTel attributes and dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrchestratorKind {
    SingleAgent,
    Conductor,
    Trinity,
    AbMcts,
    SelfCaller,
}

#[async_trait]
pub trait Orchestrator: Send + Sync + 'static {
    fn kind(&self) -> OrchestratorKind;

    async fn run(&self, principal: &Principal, input: OrchInput) -> Result<OrchOutput, TakoError>;

    async fn stream(
        &self,
        principal: &Principal,
        input: OrchInput,
    ) -> BoxStream<'static, Result<OrchEvent, TakoError>>;
}
