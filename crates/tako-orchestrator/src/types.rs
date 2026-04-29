//! Common types shared by orchestrators.

use serde::{Deserialize, Serialize};
use tako_core::{Message, Usage};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchInput {
    /// Initial conversation seed. Most callers pass a single user message.
    pub messages: Vec<Message>,
    #[serde(default)]
    pub system: Option<String>,
}

impl OrchInput {
    pub fn from_user(text: impl Into<String>) -> Self {
        Self {
            messages: vec![Message::user(text)],
            system: None,
        }
    }

    pub fn with_system(mut self, text: impl Into<String>) -> Self {
        self.system = Some(text.into());
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchOutput {
    /// Concatenated assistant text from the final turn.
    pub text: String,
    /// Full final assistant message (tool calls included if loop hit step cap).
    pub message: Message,
    /// Cumulative usage across all provider calls.
    pub usage: Usage,
    /// Number of provider calls (i.e. assistant turns produced).
    pub steps: u32,
}

/// Streaming event emitted by an [`Orchestrator::stream`] implementation.
///
/// Marked `#[non_exhaustive]` from v0.9.0 onwards so future additive
/// variants don't force another minor-version break. Consumers that
/// match on the enum should always include a wildcard arm.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum OrchEvent {
    StepStart {
        step: u32,
    },
    AssistantText {
        step: u32,
        delta: String,
    },
    ToolCallStart {
        step: u32,
        name: String,
        id: String,
    },
    ToolCallResult {
        step: u32,
        id: String,
        result: serde_json::Value,
        is_error: bool,
    },
    Final {
        output: Box<OrchOutput>,
    },
    /// Per-rollout verifier score from a search-style orchestrator
    /// (introduced for AB-MCTS native streaming in v0.9.0). `step` is
    /// the rollout iteration; `branch` identifies the expansion branch
    /// the score was computed against; `score` is the verifier output
    /// in `[0.0, 1.0]`.
    VerifierScore {
        step: u32,
        branch: u32,
        score: f32,
    },
    /// Recursion-boundary signal from a self-recursive orchestrator
    /// (introduced for `SelfCaller` streaming-aware confidence guards
    /// in v0.9.0). `depth` is the current recursion depth (0-indexed);
    /// `confidence` is the guard's score for the iteration just
    /// completed (or the early-abort score for the streaming-aware
    /// path).
    Recursion {
        depth: u32,
        confidence: f32,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn verifier_score_serde_roundtrip() {
        let ev = OrchEvent::VerifierScore {
            step: 3,
            branch: 1,
            score: 0.875,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains(r#""kind":"verifier_score""#));
        assert!(s.contains(r#""step":3"#));
        assert!(s.contains(r#""branch":1"#));
        let back: OrchEvent = serde_json::from_str(&s).unwrap();
        match back {
            OrchEvent::VerifierScore {
                step,
                branch,
                score,
            } => {
                assert_eq!(step, 3);
                assert_eq!(branch, 1);
                assert!((score - 0.875).abs() < 1e-6);
            }
            _ => panic!("expected VerifierScore"),
        }
    }

    #[test]
    fn recursion_serde_roundtrip() {
        let ev = OrchEvent::Recursion {
            depth: 2,
            confidence: 0.42,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains(r#""kind":"recursion""#));
        assert!(s.contains(r#""depth":2"#));
        let back: OrchEvent = serde_json::from_str(&s).unwrap();
        match back {
            OrchEvent::Recursion { depth, confidence } => {
                assert_eq!(depth, 2);
                assert!((confidence - 0.42).abs() < 1e-6);
            }
            _ => panic!("expected Recursion"),
        }
    }
}
