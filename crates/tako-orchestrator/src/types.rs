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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
}
