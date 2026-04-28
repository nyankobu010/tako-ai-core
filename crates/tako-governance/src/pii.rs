//! PII / DLP redaction. Presidio-style regex set + Luhn for credit cards,
//! plus a [`ContentTransform`] enum to choose how matches are handled.
//!
//! The regex literals are static, validated at module-load time, and only
//! ever fail under a programmer error: hence the `expect`s in the `OnceLock`
//! initialiser are intentional and documented here.
#![allow(clippy::expect_used)]

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use std::sync::OnceLock;

/// What to do when a match is found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentTransform {
    /// Replace each character of the match with `*`.
    Mask,
    /// Replace the match with the SHA-256 hex digest of its contents.
    HashSha256,
    /// Replace the match with `[REDACTED:<kind>]`.
    Redact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiKind {
    Email,
    Phone,
    Ipv4,
    AwsKey,
    Jwt,
    Ssn,
    CreditCard,
}

impl PiiKind {
    fn label(self) -> &'static str {
        match self {
            Self::Email => "EMAIL",
            Self::Phone => "PHONE",
            Self::Ipv4 => "IPV4",
            Self::AwsKey => "AWS_KEY",
            Self::Jwt => "JWT",
            Self::Ssn => "SSN",
            Self::CreditCard => "CREDIT_CARD",
        }
    }
}

struct CompiledRule {
    kind: PiiKind,
    re: Regex,
    /// Optional post-match validator (used by credit_card → Luhn).
    validate: Option<fn(&str) -> bool>,
}

fn compiled_rules() -> &'static [CompiledRule] {
    static CELL: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    CELL.get_or_init(|| {
        vec![
            CompiledRule {
                kind: PiiKind::Email,
                re: Regex::new(r"\b[\w._%+-]+@[\w.-]+\.[A-Za-z]{2,24}\b").expect("valid regex"),
                validate: None,
            },
            CompiledRule {
                kind: PiiKind::AwsKey,
                re: Regex::new(r"\b(AKIA|ASIA)[0-9A-Z]{16}\b").expect("valid regex"),
                validate: None,
            },
            CompiledRule {
                kind: PiiKind::Jwt,
                re: Regex::new(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b").expect("valid regex"),
                validate: None,
            },
            CompiledRule {
                kind: PiiKind::Ssn,
                re: Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").expect("valid regex"),
                validate: None,
            },
            CompiledRule {
                kind: PiiKind::CreditCard,
                re: Regex::new(r"\b(?:\d[ -]?){13,19}\b").expect("valid regex"),
                validate: Some(luhn_check),
            },
            CompiledRule {
                kind: PiiKind::Phone,
                re: Regex::new(r"\b\+?\d{1,3}[ -]?\(?\d{1,4}\)?[ -]?\d{3,4}[ -]?\d{4}\b").expect("valid regex"),
                validate: None,
            },
            CompiledRule {
                kind: PiiKind::Ipv4,
                re: Regex::new(
                    r"\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d?\d)\b",
                )
                .expect("valid regex"),
                validate: None,
            },
        ]
    })
    .as_slice()
}

fn luhn_check(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum: u32 = 0;
    for (i, d) in digits.iter().rev().enumerate() {
        let mut x = *d;
        if i % 2 == 1 {
            x *= 2;
            if x > 9 {
                x -= 9;
            }
        }
        sum += x;
    }
    sum % 10 == 0
}

/// Found PII span.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiiHit {
    pub kind: PiiKind,
    pub start: usize,
    pub end: usize,
}

/// Scan `input` for PII. Returns hits in input order.
pub fn detect(input: &str) -> Vec<PiiHit> {
    let mut hits: Vec<PiiHit> = Vec::new();
    for rule in compiled_rules() {
        for m in rule.re.find_iter(input) {
            if let Some(v) = rule.validate {
                if !v(m.as_str()) {
                    continue;
                }
            }
            hits.push(PiiHit { kind: rule.kind, start: m.start(), end: m.end() });
        }
    }
    // Resolve overlaps: keep the earliest, then break ties by longer.
    hits.sort_by_key(|h| (h.start, std::cmp::Reverse(h.end - h.start)));
    let mut out: Vec<PiiHit> = Vec::with_capacity(hits.len());
    let mut cursor = 0;
    for h in hits {
        if h.start >= cursor {
            cursor = h.end;
            out.push(h);
        }
    }
    out
}

/// Apply a [`ContentTransform`] to all detected PII in `input`.
pub fn apply(input: &str, transform: ContentTransform) -> String {
    let hits = detect(input);
    if hits.is_empty() {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    for hit in hits {
        out.push_str(&input[cursor..hit.start]);
        let raw = &input[hit.start..hit.end];
        match transform {
            ContentTransform::Mask => out.extend(raw.chars().map(|_| '*')),
            ContentTransform::HashSha256 => {
                let mut h = Sha256::new();
                h.update(raw.as_bytes());
                let digest = h.finalize();
                out.push_str(&hex(&digest));
            }
            ContentTransform::Redact => {
                out.push_str(&format!("[REDACTED:{}]", hit.kind.label()));
            }
        }
        cursor = hit.end;
    }
    out.push_str(&input[cursor..]);
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn detects_email_and_aws_key() {
        let s = "Reach out to alice@acme.org with key AKIA1234567890ABCDEF.";
        let hits = detect(s);
        let kinds: Vec<PiiKind> = hits.iter().map(|h| h.kind).collect();
        assert!(kinds.contains(&PiiKind::Email));
        assert!(kinds.contains(&PiiKind::AwsKey));
    }

    #[test]
    fn redact_replaces_with_label() {
        let out = apply("from alice@acme.org", ContentTransform::Redact);
        assert_eq!(out, "from [REDACTED:EMAIL]");
    }

    #[test]
    fn mask_uses_stars() {
        let out = apply("alice@acme.org", ContentTransform::Mask);
        assert_eq!(out, "**************");
    }

    #[test]
    fn luhn_filters_invalid_card_numbers() {
        // 4111-1111-1111-1111 is a valid Luhn test card.
        let valid = apply("card 4111-1111-1111-1111", ContentTransform::Redact);
        assert!(valid.contains("[REDACTED:CREDIT_CARD]"));
        // 1234567890123456 fails Luhn.
        let invalid = apply("number 1234567890123456", ContentTransform::Redact);
        assert!(!invalid.contains("REDACTED"));
    }

    #[test]
    fn hash_is_deterministic() {
        let a = apply("alice@acme.org", ContentTransform::HashSha256);
        let b = apply("alice@acme.org", ContentTransform::HashSha256);
        assert_eq!(a, b);
        assert_ne!(a, "alice@acme.org");
    }

    #[test]
    fn jwt_is_redacted() {
        let s = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signature123";
        let out = apply(s, ContentTransform::Redact);
        assert!(out.contains("[REDACTED:JWT]"));
    }
}
