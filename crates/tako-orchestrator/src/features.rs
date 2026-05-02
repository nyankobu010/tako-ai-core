//! Shared feature extractor for routing classifiers.
//!
//! Produces a fixed-length `Vec<f32>` from a `ChatRequest` so that
//! `RegexRouter` and `OnnxRouter` operate on the same input, and the
//! Python training harness in `python/tako/training/features.py` can
//! reproduce identical vectors. Parity is asserted by
//! `tests/data/featuriser_corpus.jsonl` + a parity test in
//! `tests/python/test_trinity.py`.
//!
//! The feature set is intentionally tiny (16 dims) so a 2-layer MLP
//! head can train on a few hundred rollouts.

use tako_core::{ChatRequest, ContentPart, Message, Role};

/// Number of features the extractor produces. Pinned so trained ONNX
/// models stay valid across tako versions.
pub const FEATURE_DIM: usize = 16;

/// Extract features from the most recent user message in `req`.
///
/// Falls back to an all-zeros vector if no user message is present.
pub fn featurise(req: &ChatRequest) -> Vec<f32> {
    let text = last_user_text(&req.messages).unwrap_or_default();
    featurise_text(&text)
}

/// Public-facing variant that takes raw text. Used by the training
/// harness during rollout generation.
pub fn featurise_text(text: &str) -> Vec<f32> {
    let mut f = vec![0.0_f32; FEATURE_DIM];
    let lower = text.to_lowercase();
    let bytes = text.as_bytes();
    let len = bytes.len() as f32;

    // 0: log10(1 + char count). Captures long-vs-short prompts.
    f[0] = (1.0 + len).log10();
    // 1: word count / 100, clamped to [0, 1]. Saturates at ~100 words.
    let words = text.split_whitespace().count() as f32;
    f[1] = (words / 100.0).min(1.0);
    // 2: question mark presence.
    f[2] = if text.contains('?') { 1.0 } else { 0.0 };
    // 3: code-block presence (``` fence or ` inline).
    f[3] = if text.contains("```") || text.contains('`') {
        1.0
    } else {
        0.0
    };
    // 4: code-keyword density (simple boolean OR on programming hints).
    let code_kw = [
        "fn ",
        "def ",
        "class ",
        "import ",
        "function ",
        "return ",
        "let ",
        "const ",
    ];
    f[4] = if code_kw.iter().any(|kw| lower.contains(kw)) {
        1.0
    } else {
        0.0
    };
    // 5: math-symbol presence.
    let math_chars = ['=', '+', '*', '/', '^', '∫', '∑', '√'];
    f[5] = if text.chars().any(|c| math_chars.contains(&c)) {
        1.0
    } else {
        0.0
    };
    // 6: digit density.
    let digits = bytes.iter().filter(|b| b.is_ascii_digit()).count() as f32;
    f[6] = if len > 0.0 { digits / len } else { 0.0 };
    // 7: uppercase density.
    let upper = bytes.iter().filter(|b| b.is_ascii_uppercase()).count() as f32;
    f[7] = if len > 0.0 { upper / len } else { 0.0 };
    // 8: presence of "code" keyword (verbatim).
    f[8] = if lower.contains("code") { 1.0 } else { 0.0 };
    // 9: presence of "math" or "solve" keyword.
    f[9] = if lower.contains("math") || lower.contains("solve") {
        1.0
    } else {
        0.0
    };
    // 10: presence of "explain" / "describe".
    f[10] = if lower.contains("explain") || lower.contains("describe") {
        1.0
    } else {
        0.0
    };
    // 11: presence of "verify" / "check" / "prove".
    f[11] = if lower.contains("verify") || lower.contains("check") || lower.contains("prove") {
        1.0
    } else {
        0.0
    };
    // 12: number of newlines normalised by 20.
    let newlines = bytes.iter().filter(|b| **b == b'\n').count() as f32;
    f[12] = (newlines / 20.0).min(1.0);
    // 13: punctuation density.
    let punct = bytes
        .iter()
        .filter(|b| matches!(**b, b'.' | b',' | b';' | b':' | b'!'))
        .count() as f32;
    f[13] = if len > 0.0 { punct / len } else { 0.0 };
    // 14: parens balance flag (present and matching count).
    let opens = bytes.iter().filter(|b| **b == b'(').count();
    let closes = bytes.iter().filter(|b| **b == b')').count();
    f[14] = if opens > 0 && opens == closes {
        1.0
    } else {
        0.0
    };
    // 15: bias term (constant 1.0).
    f[15] = 1.0;
    f
}

fn last_user_text(messages: &[Message]) -> Option<String> {
    for m in messages.iter().rev() {
        if matches!(m.role, Role::User) {
            let t = m
                .content
                .iter()
                .filter_map(ContentPart::as_text)
                .collect::<Vec<_>>()
                .join("\n");
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn feature_dim_constant() {
        assert_eq!(featurise_text("anything").len(), FEATURE_DIM);
        assert_eq!(featurise_text("").len(), FEATURE_DIM);
    }

    #[test]
    fn empty_input_returns_zero_vector_except_bias() {
        let f = featurise_text("");
        for (i, v) in f.iter().enumerate().take(FEATURE_DIM - 1) {
            assert_eq!(*v, 0.0, "feature {i} should be zero on empty input");
        }
        assert_eq!(f[FEATURE_DIM - 1], 1.0, "bias should be 1");
    }

    #[test]
    fn code_prompt_lights_code_features() {
        let f = featurise_text("Please show me code in Rust.\n```rust\nfn x() {}\n```");
        assert_eq!(f[3], 1.0); // code-block fence
        assert_eq!(f[4], 1.0); // code-keyword (fn )
        assert_eq!(f[8], 1.0); // 'code' substring
    }

    #[test]
    fn math_prompt_lights_math_features() {
        let f = featurise_text("Solve x^2 + 5x + 6 = 0");
        assert_eq!(f[5], 1.0); // math symbols
        assert_eq!(f[9], 1.0); // 'solve'
    }

    #[test]
    fn last_user_text_reads_most_recent_user_msg() {
        let req = ChatRequest::new(
            "m",
            vec![
                Message::user("first"),
                Message::assistant("a"),
                Message::user("most recent"),
            ],
        );
        let f1 = featurise(&req);
        let f2 = featurise_text("most recent");
        assert_eq!(f1, f2);
    }
}
