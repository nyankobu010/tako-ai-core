//! Built-in `Verifier` impls for AB-MCTS.
//!
//! Mirrors the `RuleBasedGuard` / `LlmJudgeGuard` pair shipped in Phase 3
//! for `SelfCaller`, but for the `Verifier` trait (which sees both the
//! prompt AND the rollout output, not just the output text).

use std::sync::Arc;

use async_trait::async_trait;
use tako_core::{
    ChatRequest, ContentPart, FinishReason, LlmProvider, Message, Principal, TakoError, Verifier,
};

/// Rule-based verifier: scores 1.0 iff the output is at least
/// `min_chars` long and (optionally) matches a regex; otherwise scores
/// proportionally to how many rules pass. Cheap default for CI smoke
/// runs and AB-MCTS exploration tests.
#[derive(Debug)]
pub struct RuleBasedVerifier {
    min_chars: usize,
    pattern: Option<regex::Regex>,
}

impl RuleBasedVerifier {
    pub fn new(min_chars: usize) -> Self {
        Self {
            min_chars,
            pattern: None,
        }
    }

    pub fn with_pattern(mut self, pat: regex::Regex) -> Self {
        self.pattern = Some(pat);
        self
    }
}

#[async_trait]
impl Verifier for RuleBasedVerifier {
    async fn score(
        &self,
        _principal: &Principal,
        _prompt: &str,
        output: &str,
    ) -> Result<f32, TakoError> {
        Ok(self.score_text(output))
    }

    /// Phase 13.B — streaming-aware variant. The same length / regex
    /// rules apply to the cumulative partial buffer; emitting on every
    /// delta is essentially free for this verifier (no remote calls,
    /// no allocation hot-path), so we always return `Ok(Some(score))`
    /// rather than gating with throttling state. Mirrors the
    /// [`crate::RuleBasedGuard`] streaming pattern shipped in Phase 8.D.
    async fn evaluate_streaming(
        &self,
        _principal: &Principal,
        partial: &str,
    ) -> Result<Option<f32>, TakoError> {
        Ok(Some(self.score_text(partial)))
    }
}

impl RuleBasedVerifier {
    fn score_text(&self, output: &str) -> f32 {
        let mut total = 1u32; // length rule always counted
        let mut passed = 0u32;
        if output.len() >= self.min_chars {
            passed += 1;
        }
        if let Some(re) = &self.pattern {
            total += 1;
            if re.is_match(output) {
                passed += 1;
            }
        }
        passed as f32 / total as f32
    }
}

/// LLM-as-judge verifier: asks another provider to score the
/// `(prompt, output)` pair on `[0, 1]`. The judge prompt asks for a
/// single decimal; unparseable replies fall back to `0.5`.
pub struct LlmJudgeVerifier {
    judge: Arc<dyn LlmProvider>,
    rubric: String,
}

impl std::fmt::Debug for LlmJudgeVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmJudgeVerifier")
            .field("judge", &self.judge.id())
            .finish()
    }
}

impl LlmJudgeVerifier {
    pub fn new(judge: Arc<dyn LlmProvider>, rubric: impl Into<String>) -> Self {
        Self {
            judge,
            rubric: rubric.into(),
        }
    }
}

#[async_trait]
impl Verifier for LlmJudgeVerifier {
    async fn score(
        &self,
        principal: &Principal,
        prompt: &str,
        output: &str,
    ) -> Result<f32, TakoError> {
        let judge_prompt = format!(
            "{}\n\nThe original task was:\n---\n{}\n---\n\nThe candidate answer is:\n---\n{}\n---\n\n\
             Reply with ONLY a decimal between 0 and 1 representing your assessment of the answer's \
             quality. No other text.",
            self.rubric, prompt, output,
        );
        let model = self.judge.id().split(':').nth(1).unwrap_or("").to_string();
        let req = ChatRequest::new(model, vec![Message::user(judge_prompt)]);
        let resp = self.judge.chat(principal, req).await?;
        if !matches!(
            resp.finish_reason,
            FinishReason::Stop | FinishReason::Length | FinishReason::ToolCalls
        ) {
            return Ok(0.5);
        }
        let raw = resp
            .message
            .content
            .iter()
            .filter_map(ContentPart::as_text)
            .collect::<Vec<_>>()
            .join("");
        Ok(parse_score(&raw).unwrap_or(0.5))
    }
}

fn parse_score(text: &str) -> Option<f32> {
    text.split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .filter(|s| !s.is_empty())
        .find_map(|s| s.parse::<f32>().ok())
        .map(|f| f.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[tokio::test]
    async fn rule_based_verifier_partial_score() {
        // No pattern: only length rule. Passes if min_chars met → 1.0,
        // else 0.0.
        let v = RuleBasedVerifier::new(10);
        let p = Principal::anonymous();
        assert_eq!(v.score(&p, "?", "short").await.unwrap(), 0.0);
        assert_eq!(v.score(&p, "?", "long enough!").await.unwrap(), 1.0);
    }

    #[tokio::test]
    async fn rule_based_verifier_grades_partial_match() {
        // Two rules: length + pattern. Half-credit when only one passes.
        let v = RuleBasedVerifier::new(5).with_pattern(regex::Regex::new(r"\bdone\b").unwrap());
        let p = Principal::anonymous();
        // Both rules pass.
        assert_eq!(v.score(&p, "?", "all done here").await.unwrap(), 1.0);
        // Only length passes.
        assert_eq!(v.score(&p, "?", "not finished").await.unwrap(), 0.5);
        // Neither passes.
        assert_eq!(v.score(&p, "?", "no").await.unwrap(), 0.0);
    }

    /// Phase 13.B — streaming variant returns `Some(score)` mirroring
    /// `score()` semantics on the cumulative partial. Cheap-heuristic
    /// verifiers can emit a score per delta without throttling.
    #[tokio::test]
    async fn rule_based_verifier_evaluate_streaming_emits_score() {
        let v = RuleBasedVerifier::new(10);
        let p = Principal::anonymous();
        // Below threshold: length rule fails -> 0.0 (still emitted).
        assert_eq!(v.evaluate_streaming(&p, "short").await.unwrap(), Some(0.0));
        // At/above threshold: length rule passes -> 1.0.
        assert_eq!(
            v.evaluate_streaming(&p, "long enough!").await.unwrap(),
            Some(1.0)
        );
    }

    #[test]
    fn parse_score_extracts_first_number() {
        assert_eq!(parse_score("0.7"), Some(0.7));
        assert_eq!(parse_score("Score: 0.85"), Some(0.85));
        assert_eq!(parse_score("nope"), None);
    }
}
