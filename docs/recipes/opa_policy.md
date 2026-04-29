# Recipe: OPA / Rego policy

Block tool calls or model invocations based on Rego rules, with every
decision audited.

## A worked Rego rule

```rego
package tako.policy

default allow = true

# Block shell.exec for tenants without admin role
deny[msg] {
    input.stage == "pre_tool"
    input.tool == "shell.exec"
    not input.principal.roles[_] == "admin"
    msg := sprintf("shell.exec requires admin (tenant %v)", [input.principal.tenant_id])
}

# Force gpt-3.5 for free-tier tenants
force_model[model] {
    input.stage == "pre_chat"
    input.principal.tenant_id == "free"
    model := "gpt-3.5-turbo"
}
```

## Wire it into a SingleAgent

```rust
use tako_governance::{AuditLog, OpaBundle};

let bundle = OpaBundle::from_path("policies/my_rules.rego")?;
let audit = AuditLog::jsonl("/var/log/tako/audit.jsonl")?;

let agent = SingleAgent::builder()
    .provider(provider)
    .policy(bundle.into_engine_with_audit(audit))
    .build()?;
```

## Three enforcement stages

| Stage | Input fields | Decision space |
|-------|-------------|----------------|
| `pre_chat` | `principal`, `model`, `messages_hash`, `tools` | `Allow` / `Deny` / `RedactMessages` / `ForceModel` |
| `pre_tool` | `principal`, `tool`, `args_hash` | `Allow` / `Deny` / `RequireApproval` |
| `post_chat` | `principal`, `response_hash` | `Allow` / `RedactResponse` |

A `Deny` propagates as `TakoError::PolicyDenied`. `RequireApproval`
short-circuits with the same error today (Phase 3 will add a real
approval flow).

## Audit log shape

`AuditLog::jsonl(path)` writes one JSONL line per decision:

```json
{
  "ts": "2026-04-29T17:32:04.123Z",
  "principal": {"tenant_id": "acme", "user_id": "alice", "roles": []},
  "stage": "pre_tool",
  "tool": "shell.exec",
  "decision": "deny",
  "reason": "shell.exec requires admin (tenant acme)"
}
```

The JSONL format is intentionally stable; Phase-4 SIEM exporters will
read the same shape.

## Bundle hashing & caching

`OpaBundle::from_string(...)` computes SHA-256 of the source. The
compiled `regorus::Engine` is cached by hash in a process-global
`Mutex<HashMap<[u8;32], Arc<Engine>>>`, so re-creating an `OpaBundle`
from the same source is cheap.

`OpaBundle::from_path` watches the file's `mtime` and recompiles only
when it changes — good for hot-reload via SIGHUP.
