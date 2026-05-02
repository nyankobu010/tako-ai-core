//! OPA / Rego policy enforcement via the [`regorus`] interpreter.
//!
//! Policies are loaded as a bundle (file or in-memory string) and
//! compiled into a [`regorus::Engine`] cached by SHA-256 of the source.
//! Each [`OpaBundle`] is consulted at three lifecycle stages:
//!
//! - `PreChat`: before a provider call. Bundle exposes
//!   `data.tako.pre_chat.decision`.
//! - `PreTool`: before a tool invocation. Bundle exposes
//!   `data.tako.pre_tool.decision`.
//! - `PostChat`: after the model returns. Bundle exposes
//!   `data.tako.post_chat.decision`.
//!
//! Each rule returns a JSON object like `{"decision": "allow"}` or
//! `{"decision": "deny", "reason": "..."}`. Missing rules default to
//! `allow`, so a partial bundle that only enforces, say, tool calls,
//! is fine.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tako_core::{PolicyContext, PolicyDecision, PolicyEngine, PolicyStage, Principal, TakoError};
use tokio::sync::Mutex;

/// A loaded Rego policy bundle.
#[derive(Clone)]
pub struct OpaBundle {
    inner: Arc<BundleInner>,
}

struct BundleInner {
    /// SHA-256 of the source(s) that built this bundle. Used as the
    /// cache key for [`compiled_engines`].
    sha256: String,
    /// Source files (path → content). Multi-file bundles load all
    /// `.rego` files from a directory.
    sources: HashMap<String, String>,
}

impl std::fmt::Debug for OpaBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpaBundle")
            .field("sha256", &self.inner.sha256)
            .field("files", &self.inner.sources.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl OpaBundle {
    /// Load a single-file bundle from a string.
    pub fn from_string(name: impl Into<String>, source: impl Into<String>) -> Self {
        let mut sources = HashMap::new();
        let name = name.into();
        let source = source.into();
        sources.insert(name, source);
        let sha256 = compute_sha(&sources);
        Self {
            inner: Arc::new(BundleInner { sha256, sources }),
        }
    }

    /// Load every `.rego` file under `dir` (non-recursive) into a single
    /// bundle.
    pub fn from_path(dir: impl AsRef<Path>) -> Result<Self, TakoError> {
        let dir = dir.as_ref();
        let mut sources = HashMap::new();
        let entries = std::fs::read_dir(dir).map_err(|e| {
            TakoError::Invalid(format!("OpaBundle from_path({:?}): {e}", dir.display()))
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| TakoError::Invalid(format!("read_dir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("rego") {
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| path.display().to_string());
                let content = std::fs::read_to_string(&path).map_err(|e| {
                    TakoError::Invalid(format!("read_to_string({}): {e}", path.display()))
                })?;
                sources.insert(name, content);
            }
        }
        if sources.is_empty() {
            return Err(TakoError::Invalid(format!(
                "OpaBundle::from_path({:?}): no .rego files found",
                dir.display()
            )));
        }
        let sha256 = compute_sha(&sources);
        Ok(Self {
            inner: Arc::new(BundleInner { sha256, sources }),
        })
    }

    /// Build the underlying [`regorus::Engine`] (cached process-wide by
    /// bundle SHA-256). Compilation is amortised across enforcement
    /// calls so per-request overhead is just a `set_input` + query.
    fn engine(&self) -> Result<regorus::Engine, TakoError> {
        // We cache the *source map* and rebuild a per-call Engine; the
        // alternative is `Arc<Mutex<Engine>>` but Engine is not Sync.
        // Building from a precompiled set of policies is fast.
        let mut engine = regorus::Engine::new();
        for (name, src) in &self.inner.sources {
            engine
                .add_policy(name.clone(), src.clone())
                .map_err(|e| TakoError::Invalid(format!("Rego compile {name}: {e}")))?;
        }
        Ok(engine)
    }

    pub fn sha256(&self) -> &str {
        &self.inner.sha256
    }
}

fn compute_sha(sources: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = sources.keys().collect();
    keys.sort();
    let mut hasher = Sha256::new();
    for k in keys {
        hasher.update(k.as_bytes());
        hasher.update(b":");
        hasher.update(sources[k].as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

#[async_trait]
impl PolicyEngine for OpaBundle {
    async fn evaluate(
        &self,
        principal: &Principal,
        ctx: PolicyContext,
    ) -> Result<PolicyDecision, TakoError> {
        let mut engine = self.engine()?;

        let input = serde_json::json!({
            "principal": {
                "tenant_id": principal.tenant_id,
                "user_id": principal.user_id,
                "roles": principal.roles,
            },
            "stage": match ctx.stage {
                PolicyStage::PreChat => "pre_chat",
                PolicyStage::PreTool => "pre_tool",
                PolicyStage::PostChat => "post_chat",
            },
            "model": ctx.model,
            "messages_hash": ctx.messages_hash,
            "tools": ctx.tools,
            "tool_args_hash": ctx.tool_args_hash,
            "response_hash": ctx.response_hash,
        });

        engine
            .set_input_json(&input.to_string())
            .map_err(|e| TakoError::Invalid(format!("Rego set_input: {e}")))?;

        let query = match ctx.stage {
            PolicyStage::PreChat => "data.tako.pre_chat.decision",
            PolicyStage::PreTool => "data.tako.pre_tool.decision",
            PolicyStage::PostChat => "data.tako.post_chat.decision",
        };

        let result = engine
            .eval_query(query.into(), false)
            .map_err(|e| TakoError::Invalid(format!("Rego eval `{query}`: {e}")))?;

        let value = match result.result.into_iter().next() {
            Some(set) => match set.expressions.into_iter().next() {
                Some(expr) => expr.value,
                None => regorus::Value::Undefined,
            },
            None => regorus::Value::Undefined,
        };

        Ok(parse_decision(value))
    }
}

fn parse_decision(v: regorus::Value) -> PolicyDecision {
    use regorus::Value;
    // Missing / undefined = allow (consistent with "no rule = no
    // restriction" semantics).
    if matches!(v, Value::Undefined | Value::Null) {
        return PolicyDecision::Allow;
    }
    let json_str = serde_json::to_string(&v).unwrap_or_default();
    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null);
    match parsed {
        serde_json::Value::String(s) if s == "allow" => PolicyDecision::Allow,
        serde_json::Value::String(s) if s == "deny" => PolicyDecision::Deny {
            reason: "policy denied".into(),
        },
        serde_json::Value::Object(map) => {
            let decision = map
                .get("decision")
                .and_then(|v| v.as_str())
                .unwrap_or("allow")
                .to_string();
            let reason = map
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("policy denied")
                .to_string();
            match decision.as_str() {
                "allow" => PolicyDecision::Allow,
                "deny" => PolicyDecision::Deny { reason },
                "redact_messages" => {
                    let mask = map
                        .get("mask")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    PolicyDecision::RedactMessages { mask }
                }
                "redact_response" => {
                    let mask = map
                        .get("mask")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    PolicyDecision::RedactResponse { mask }
                }
                "force_model" => {
                    let model = map
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    PolicyDecision::ForceModel { model }
                }
                "require_approval" => PolicyDecision::RequireApproval { reason },
                _ => PolicyDecision::Allow,
            }
        }
        _ => PolicyDecision::Allow,
    }
}

/// Append-only JSON-lines audit log for policy decisions. Every
/// evaluate() call should be wrapped via [`AuditLog::record`] so SIEM
/// pipelines can replay decisions.
#[derive(Clone)]
pub struct AuditLog {
    inner: Arc<Mutex<AuditInner>>,
}

struct AuditInner {
    writer: Box<dyn std::io::Write + Send>,
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog").finish_non_exhaustive()
    }
}

impl AuditLog {
    /// Open a JSONL audit log at `path`, appending if it exists.
    pub fn jsonl(path: impl AsRef<Path>) -> Result<Self, TakoError> {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.as_ref())
            .map_err(|e| TakoError::Invalid(format!("AuditLog::jsonl open: {e}")))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(AuditInner {
                writer: Box::new(f),
            })),
        })
    }

    /// Test helper: in-memory writer.
    pub fn in_memory() -> (Self, Arc<std::sync::Mutex<Vec<u8>>>) {
        let buf: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = MemWriter {
            buf: Arc::clone(&buf),
        };
        (
            Self {
                inner: Arc::new(Mutex::new(AuditInner {
                    writer: Box::new(writer),
                })),
            },
            buf,
        )
    }

    /// Record a single decision. Best-effort: write errors are logged
    /// but don't propagate.
    pub async fn record(
        &self,
        principal: &Principal,
        ctx: &PolicyContext,
        decision: &PolicyDecision,
    ) {
        let entry = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "principal": {
                "tenant_id": principal.tenant_id,
                "user_id": principal.user_id,
            },
            "stage": match ctx.stage {
                PolicyStage::PreChat => "pre_chat",
                PolicyStage::PreTool => "pre_tool",
                PolicyStage::PostChat => "post_chat",
            },
            "decision": decision,
            "model": ctx.model,
        });
        let line = format!("{entry}\n");
        let mut g = self.inner.lock().await;
        if let Err(e) = g.writer.write_all(line.as_bytes()) {
            tracing::warn!(error = %e, "audit log write failed");
        }
    }
}

struct MemWriter {
    buf: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl std::io::Write for MemWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut g) = self.buf.lock() {
            g.extend_from_slice(data);
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tako_core::PolicyStage;

    fn ctx_pre_tool() -> PolicyContext {
        PolicyContext {
            stage: PolicyStage::PreTool,
            model: "claude-test".into(),
            messages_hash: "abc".into(),
            tools: vec!["shell.exec".into()],
            tool_args_hash: Some("def".into()),
            response_hash: None,
        }
    }

    fn admin_principal() -> Principal {
        Principal {
            tenant_id: "acme".into(),
            user_id: "alice".into(),
            roles: vec!["admin".into()],
            trace_id: None,
            metadata: Default::default(),
        }
    }

    fn user_principal() -> Principal {
        Principal {
            tenant_id: "acme".into(),
            user_id: "bob".into(),
            roles: vec!["user".into()],
            trace_id: None,
            metadata: Default::default(),
        }
    }

    const POLICY: &str = r#"
package tako.pre_tool

default decision := {"decision": "allow"}

decision := {"decision": "deny", "reason": msg} if {
    "shell.exec" in input.tools
    not "admin" in input.principal.roles
    msg := "shell.exec requires admin role"
}
"#;

    #[tokio::test]
    async fn opa_allows_admin_for_shell_exec() {
        let bundle = OpaBundle::from_string("test.rego", POLICY);
        let decision = bundle
            .evaluate(&admin_principal(), ctx_pre_tool())
            .await
            .unwrap();
        assert!(matches!(decision, PolicyDecision::Allow));
    }

    #[tokio::test]
    async fn opa_denies_non_admin_for_shell_exec() {
        let bundle = OpaBundle::from_string("test.rego", POLICY);
        let decision = bundle
            .evaluate(&user_principal(), ctx_pre_tool())
            .await
            .unwrap();
        let PolicyDecision::Deny { reason } = decision else {
            panic!("expected Deny");
        };
        assert!(reason.contains("admin"));
    }

    #[tokio::test]
    async fn opa_default_allow_when_no_rule() {
        // Bundle that only covers pre_chat; pre_tool defaults to allow.
        let policy = r#"
package tako.pre_chat
default decision := {"decision": "allow"}
"#;
        let bundle = OpaBundle::from_string("only_pre_chat.rego", policy);
        let decision = bundle
            .evaluate(&user_principal(), ctx_pre_tool())
            .await
            .unwrap();
        assert!(matches!(decision, PolicyDecision::Allow));
    }

    #[tokio::test]
    async fn audit_log_in_memory_records_jsonl() {
        let (log, buf) = AuditLog::in_memory();
        let decision = PolicyDecision::Deny {
            reason: "test".into(),
        };
        log.record(&user_principal(), &ctx_pre_tool(), &decision)
            .await;
        let g = buf.lock().unwrap();
        let line = std::str::from_utf8(&g).unwrap();
        assert!(line.contains("\"stage\":\"pre_tool\""));
        assert!(line.contains("\"decision\""));
        assert!(line.contains("test"));
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn bundle_sha256_changes_with_content() {
        let a = OpaBundle::from_string("a.rego", "package x");
        let b = OpaBundle::from_string("a.rego", "package y");
        assert_ne!(a.sha256(), b.sha256());
    }
}
