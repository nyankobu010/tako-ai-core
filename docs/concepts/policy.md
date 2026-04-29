# Policy enforcement (OPA / Rego)

`tako-governance` wraps `regorus` 0.9 to evaluate Rego policy against
three enforcement points in the agent loop:

| Stage | Decision space |
|-------|----------------|
| `PreChat` | Allow / Deny / RedactMessages / ForceModel |
| `PreTool` | Allow / Deny / RequireApproval |
| `PostChat` | Allow / RedactResponse |

A `Deny` propagates as `TakoError::PolicyDenied`; `RequireApproval`
short-circuits with the same error today (Phase 3 will add an approval
flow).

## Bundle a policy

```rust
use tako_governance::OpaBundle;
let bundle = OpaBundle::from_string(r#"
package tako.policy

default allow = true

# Block shell.exec for tenants without admin role
deny[msg] {
    input.stage == "pre_tool"
    input.tool == "shell.exec"
    not input.principal.roles[_] == "admin"
    msg := sprintf("shell.exec requires admin (tenant %v)", [input.principal.tenant_id])
}
"#)?;
```

Bundles are content-addressed by SHA-256 of the source string; the
compiled `regorus::Engine` is cached, so re-creating an `OpaBundle`
from the same string is cheap.

## Wire it into the orchestrator

```rust
let agent = SingleAgent::builder()
    .provider(provider)
    .policy(Arc::new(bundle.into_engine()))
    .build()?;
```

Both `SingleAgent` and `Conductor` accept `Option<Arc<dyn PolicyEngine>>`.
Without a policy, the orchestrator runs through the existing `AllowAll`
impl (zero-cost: no policy calls happen at all).

## Audit log

Every policy decision is recorded to a configurable `AuditLog`. Two
backends ship:

- `AuditLog::jsonl(path)` — append-only JSONL, suitable for SIEM
  ingestion; one decision per line with timestamp, principal, stage,
  decision.
- `AuditLog::in_memory()` — for tests.

The JSONL format is intentionally stable; Phase-4 SIEM exporters will
read the same shape.

## See also

- [recipes/opa_policy.md](../recipes/opa_policy.md) — end-to-end
  walkthrough including a worked Rego rule.
- [examples/policies/allow_with_audit.rego](https://github.com/TODO(<org>)/tako-ai-core/blob/main/examples/policies/allow_with_audit.rego)
  — the default 'allow everything but record it' bundle.
