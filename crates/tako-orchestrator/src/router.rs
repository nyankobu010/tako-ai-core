//! Concrete `Router` impls.
//!
//! - [`RegexRouter`]: rule-based default. Maps prompt features (via the
//!   shared featuriser in [`crate::features`]) to a single candidate from
//!   the candidate list.
//! - [`OnnxRouter`]: feature-gated learned router. Loads an ONNX
//!   classifier via the `ort` crate and argmaxes over candidate scores.
//!   Behind the `onnx` Cargo feature.

use std::sync::Arc;

use async_trait::async_trait;
use tako_core::{ChatRequest, Principal, Router, RoutingDecision, TakoError};
use tracing::{Instrument, info_span};

use crate::features::featurise;

/// One rule fired against the feature vector. Returns either `Some(idx)`
/// where `idx` is the candidate index to pick, or `None` to abstain.
type Rule = Arc<dyn Fn(&[f32]) -> Option<usize> + Send + Sync + 'static>;

/// Rule-based default router. Tries each rule in order; the first rule
/// that returns `Some(idx)` wins. If no rule fires, falls back to the
/// `default_idx` candidate. If the candidate list is shorter than the
/// returned index, falls back to index 0.
///
/// Built-in defaults (constructed via `RegexRouter::default()`):
/// - if features show "code" keyword/codeblock → candidate 0
/// - if features show "math" / "solve" / math symbols → candidate 1
/// - otherwise → candidate `default_idx` (defaults to 2)
///
/// The defaults assume a Trinity-style three-candidate layout
/// `[code, math, fallback]` but you can build a custom rule chain
/// with `RegexRouter::builder()`.
pub struct RegexRouter {
    rules: Vec<Rule>,
    default_idx: usize,
}

impl std::fmt::Debug for RegexRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegexRouter")
            .field("rule_count", &self.rules.len())
            .field("default_idx", &self.default_idx)
            .finish()
    }
}

impl Default for RegexRouter {
    fn default() -> Self {
        Self::builder()
            // 0: code (f[3] code-block || f[4] code-keyword || f[8] "code")
            .rule(|f| {
                if f.get(3).copied().unwrap_or(0.0) > 0.5
                    || f.get(4).copied().unwrap_or(0.0) > 0.5
                    || f.get(8).copied().unwrap_or(0.0) > 0.5
                {
                    Some(0)
                } else {
                    None
                }
            })
            // 1: math (f[5] math symbols || f[9] math keyword)
            .rule(|f| {
                if f.get(5).copied().unwrap_or(0.0) > 0.5 || f.get(9).copied().unwrap_or(0.0) > 0.5
                {
                    Some(1)
                } else {
                    None
                }
            })
            .default_idx(2)
            .build()
    }
}

impl RegexRouter {
    pub fn builder() -> RegexRouterBuilder {
        RegexRouterBuilder::default()
    }
}

#[derive(Default)]
pub struct RegexRouterBuilder {
    rules: Vec<Rule>,
    default_idx: usize,
}

impl std::fmt::Debug for RegexRouterBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegexRouterBuilder")
            .field("rule_count", &self.rules.len())
            .field("default_idx", &self.default_idx)
            .finish()
    }
}

impl RegexRouterBuilder {
    pub fn rule<F>(mut self, f: F) -> Self
    where
        F: Fn(&[f32]) -> Option<usize> + Send + Sync + 'static,
    {
        self.rules.push(Arc::new(f));
        self
    }

    pub fn default_idx(mut self, idx: usize) -> Self {
        self.default_idx = idx;
        self
    }

    pub fn build(self) -> RegexRouter {
        RegexRouter {
            rules: self.rules,
            default_idx: self.default_idx,
        }
    }
}

#[async_trait]
impl Router for RegexRouter {
    async fn route(
        &self,
        _principal: &Principal,
        req: &ChatRequest,
        candidates: &[String],
    ) -> Result<RoutingDecision, TakoError> {
        if candidates.is_empty() {
            return Err(TakoError::Invalid(
                "RegexRouter::route: candidate list is empty".into(),
            ));
        }
        let span = info_span!(
            "tako.router.route",
            "tako.router.kind" = "regex",
            "tako.router.choice" = tracing::field::Empty,
            "tako.router.confidence" = tracing::field::Empty,
        );
        let feat = featurise(req);
        let chosen = self
            .rules
            .iter()
            .find_map(|rule| rule(&feat))
            .unwrap_or(self.default_idx);
        let chosen = chosen.min(candidates.len() - 1);
        let provider_id = candidates[chosen].clone();
        // RegexRouter has no real probability; report 1.0 when a rule
        // matched, 0.5 when we fell through to the default.
        let confidence = if self
            .rules
            .iter()
            .any(|r| r(&feat).map(|i| i.min(candidates.len() - 1)) == Some(chosen))
        {
            1.0
        } else {
            0.5
        };
        span.record("tako.router.choice", provider_id.as_str());
        span.record("tako.router.confidence", confidence);
        let decision = RoutingDecision {
            provider_id,
            confidence,
            reason: Some("regex-router".into()),
        };
        async {}.instrument(span).await;
        Ok(decision)
    }
}

#[cfg(feature = "onnx")]
mod onnx_impl {
    use super::*;
    use ndarray::Array2;
    use ort::session::Session;
    use ort::value::Value;
    use std::path::{Path, PathBuf};
    use tokio::sync::Mutex;

    /// ONNX-backed router. Loads a classifier from disk, featurises each
    /// `ChatRequest`, runs inference, and argmaxes over candidate scores.
    ///
    /// The ONNX model must take a `float32[1, FEATURE_DIM]` input named
    /// `features` and emit a `float32[1, K]` output named `logits` where
    /// `K >= candidates.len()`.
    pub struct OnnxRouter {
        path: PathBuf,
        session: Mutex<Option<Session>>,
    }

    impl std::fmt::Debug for OnnxRouter {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("OnnxRouter")
                .field("path", &self.path)
                .finish()
        }
    }

    impl OnnxRouter {
        pub fn from_path(path: impl AsRef<Path>) -> Self {
            Self {
                path: path.as_ref().to_path_buf(),
                session: Mutex::new(None),
            }
        }

        async fn ensure_session(&self) -> Result<(), TakoError> {
            let mut guard = self.session.lock().await;
            if guard.is_some() {
                return Ok(());
            }
            let session = Session::builder()
                .map_err(|e| TakoError::Invalid(format!("ort builder: {e}")))?
                .commit_from_file(&self.path)
                .map_err(|e| {
                    TakoError::Invalid(format!("ort load `{}`: {e}", self.path.display()))
                })?;
            *guard = Some(session);
            Ok(())
        }
    }

    #[async_trait]
    impl Router for OnnxRouter {
        async fn route(
            &self,
            _principal: &Principal,
            req: &ChatRequest,
            candidates: &[String],
        ) -> Result<RoutingDecision, TakoError> {
            if candidates.is_empty() {
                return Err(TakoError::Invalid(
                    "OnnxRouter::route: candidate list is empty".into(),
                ));
            }
            self.ensure_session().await?;
            let feat = featurise(req);
            let mut guard = self.session.lock().await;
            let session = guard
                .as_mut()
                .ok_or_else(|| TakoError::Invalid("OnnxRouter: session missing".into()))?;
            let dim = feat.len();
            let arr = Array2::from_shape_vec((1, dim), feat)
                .map_err(|e| TakoError::Invalid(format!("ndarray shape: {e}")))?;
            let input = Value::from_array(arr)
                .map_err(|e| TakoError::Invalid(format!("ort value: {e}")))?;
            let outputs = session
                .run(ort::inputs!["features" => input])
                .map_err(|e| TakoError::Invalid(format!("ort run: {e}")))?;
            let (_shape, data) = outputs
                .get("logits")
                .ok_or_else(|| TakoError::Invalid("ort: missing 'logits' output".into()))?
                .try_extract_tensor::<f32>()
                .map_err(|e| TakoError::Invalid(format!("ort extract: {e}")))?;
            let logits: &[f32] = data;
            let n = candidates.len().min(logits.len());
            let (idx, max) = logits[..n].iter().enumerate().fold(
                (0_usize, f32::NEG_INFINITY),
                |(bi, bv), (i, v)| {
                    if *v > bv { (i, *v) } else { (bi, bv) }
                },
            );
            let confidence = softmax_at(&logits[..n], idx).unwrap_or(max);
            let span = info_span!(
                "tako.router.route",
                "tako.router.kind" = "onnx",
                "tako.router.choice" = %candidates[idx],
                "tako.router.confidence" = confidence,
            );
            let decision = RoutingDecision {
                provider_id: candidates[idx].clone(),
                confidence,
                reason: Some(format!("onnx@{}", self.path.display())),
            };
            async {}.instrument(span).await;
            Ok(decision)
        }
    }

    fn softmax_at(logits: &[f32], idx: usize) -> Option<f32> {
        let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let sum: f32 = logits.iter().map(|x| (x - max).exp()).sum();
        if sum > 0.0 {
            Some(((logits[idx] - max).exp()) / sum)
        } else {
            None
        }
    }
}

#[cfg(feature = "onnx")]
pub use onnx_impl::OnnxRouter;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use tako_core::{ChatRequest, Message, Principal};

    fn make_req(text: &str) -> ChatRequest {
        ChatRequest::new("m", vec![Message::user(text)])
    }

    #[tokio::test]
    async fn regex_router_picks_code_for_code_prompts() {
        let r = RegexRouter::default();
        let cands = vec![
            "anthropic:code".to_string(),
            "openai:math".to_string(),
            "openai:fallback".to_string(),
        ];
        let d = r
            .route(
                &Principal::anonymous(),
                &make_req("Write a fn in Rust"),
                &cands,
            )
            .await
            .unwrap();
        assert_eq!(d.provider_id, "anthropic:code");
        assert!(d.confidence > 0.0);
    }

    #[tokio::test]
    async fn regex_router_picks_math_for_math_prompts() {
        let r = RegexRouter::default();
        let cands = vec!["c".to_string(), "m".to_string(), "fb".to_string()];
        let d = r
            .route(&Principal::anonymous(), &make_req("Solve 2+2"), &cands)
            .await
            .unwrap();
        assert_eq!(d.provider_id, "m");
    }

    #[tokio::test]
    async fn regex_router_falls_back_for_chitchat() {
        let r = RegexRouter::default();
        let cands = vec!["c".to_string(), "m".to_string(), "fb".to_string()];
        let d = r
            .route(&Principal::anonymous(), &make_req("hi friend"), &cands)
            .await
            .unwrap();
        assert_eq!(d.provider_id, "fb");
        assert_eq!(d.confidence, 0.5);
    }

    #[tokio::test]
    async fn regex_router_errors_on_empty_candidates() {
        let r = RegexRouter::default();
        let err = r
            .route(&Principal::anonymous(), &make_req("hi"), &[])
            .await
            .unwrap_err();
        assert!(matches!(err, TakoError::Invalid(_)));
    }

    #[tokio::test]
    async fn regex_router_clamps_oob_default_idx() {
        let r = RegexRouter::builder().default_idx(99).build();
        let cands = vec!["only".to_string()];
        let d = r
            .route(&Principal::anonymous(), &make_req("anything"), &cands)
            .await
            .unwrap();
        assert_eq!(d.provider_id, "only");
    }

    #[cfg(feature = "onnx")]
    #[tokio::test]
    #[ignore = "requires libonnxruntime.dylib/.so at runtime; opt-in via `cargo test --ignored`"]
    async fn onnx_router_returns_error_on_missing_file() {
        let r = super::OnnxRouter::from_path("/tmp/this/does/not/exist.onnx");
        let cands = vec!["a".to_string(), "b".to_string()];
        let err = r
            .route(&Principal::anonymous(), &make_req("hi"), &cands)
            .await
            .unwrap_err();
        match err {
            TakoError::Invalid(m) => {
                assert!(m.contains("ort"), "expected ort error, got: {m}")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }
}
