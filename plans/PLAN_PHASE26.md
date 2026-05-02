# PLAN — Phase 26 (ChainedAuthResolver fail-fast on transport errors)

## Context

Phase 21 (v0.22.0, 2026-05-01) shipped `ChainedAuthResolver` with
the explicit decision that **any** `Err` from a child falls
through to the next. Rationale at the time: "transient OIDC
transport failures don't strand a static-API-key client".

That cadence is wrong for a common operator-UX scenario. Consider
the chain `OidcAuth().then(StaticTokens)`:

- OIDC issuer is healthy: every request hits OIDC first; OIDC
  authenticates the token; StaticTokens is never consulted.
- OIDC issuer is **down** (transport error): every request hits
  OIDC first; OIDC returns `TakoError::Transport(...)`; the chain
  falls through to StaticTokens; StaticTokens returns
  `"unknown bearer token"` because the user's OIDC token isn't in
  the static map.

The end-user sees a misleading 401 with `"unknown bearer token"`
when the actual problem is `"OIDC issuer unreachable"`. The
operator gets paged for a wrong-cause symptom.

The Phase 21 PLAN explicitly noted this: "If patterns emerge for
'fail fast on transport errors', a future phase may add
`with_short_circuit_on_transport_error`."

Phase 26 ships that opt-in flag.

**Theme:** *Let operators opt into transport-error short-circuit
on `ChainedAuthResolver` so misleading 401s stop masking
infrastructure failures.*

**Tag:** v0.27.0.

## A. `ChainedAuthResolver::with_short_circuit_on_transport_error`

### A.1 — Public surface

[`crates/tako-compat/src/auth/chained.rs`](crates/tako-compat/src/auth/chained.rs):

```rust
#[derive(Clone, Debug, Default)]
pub struct ChainedAuthResolver {
    children: Vec<Arc<dyn AuthResolver>>,
    /// Phase 26 — when `true`, any [`TakoError::Transport`] from
    /// a child returns immediately instead of falling through to
    /// the next child. Default `false` preserves Phase 21
    /// fall-through-on-any-Err semantics byte-for-byte.
    short_circuit_on_transport_error: bool,
}

impl ChainedAuthResolver {
    /// Phase 26 — opt in to fail-fast on transport errors. When
    /// enabled, a [`TakoError::Transport`] from any child halts
    /// the chain and propagates the error immediately, instead
    /// of falling through to the next child.
    ///
    /// Useful for the common "OIDC bearer OR static API key"
    /// pattern: when the OIDC issuer is unreachable, the operator
    /// wants the actionable `"transport error: ..."` to surface,
    /// not a misleading `"unknown bearer token"` from a fallback
    /// resolver. Other error variants
    /// ([`TakoError::Invalid`], [`TakoError::PolicyDenied`], etc.)
    /// continue to fall through — those represent auth decisions
    /// the next resolver might overturn.
    ///
    /// Idempotent. Default `false` preserves Phase 21 semantics.
    pub fn with_short_circuit_on_transport_error(mut self) -> Self {
        self.short_circuit_on_transport_error = true;
        self
    }

    /// Phase 26 — accessor for the short-circuit flag, useful for
    /// assertions in test code.
    pub fn short_circuits_on_transport_error(&self) -> bool {
        self.short_circuit_on_transport_error
    }
}
```

### A.2 — `resolve()` extension

```rust
async fn resolve(&self, token: &str) -> Result<Principal, TakoError> {
    if self.children.is_empty() {
        return Err(TakoError::Invalid(
            "chained auth: no resolvers configured".into(),
        ));
    }
    let mut last_err: Option<TakoError> = None;
    for child in &self.children {
        match child.resolve(token).await {
            Ok(p) => return Ok(p),
            Err(e) => {
                // Phase 26 — short-circuit on transport errors
                // when opted in. Other variants still fall
                // through to the next child (auth-decision
                // errors that the next resolver might overturn).
                if self.short_circuit_on_transport_error
                    && matches!(e, TakoError::Transport(_))
                {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        TakoError::Invalid("chained auth: unreachable empty-error path".into())
    }))
}
```

### A.3 — Tests

Five new unit tests in
[`crates/tako-compat/src/auth/chained.rs`](crates/tako-compat/src/auth/chained.rs):

1. `short_circuit_default_falls_through_on_transport_error` —
   Phase 21 cadence regression: without
   `with_short_circuit_on_transport_error`, transport errors
   fall through to the next child.
2. `short_circuit_enabled_returns_immediately_on_transport_error` —
   first child returns `Transport`; second child is **not**
   called; the transport error propagates.
3. `short_circuit_enabled_falls_through_on_invalid_error` —
   `TakoError::Invalid` (auth decision) still falls through
   even when short-circuit is enabled.
4. `short_circuit_enabled_first_ok_still_short_circuits_happy_path` —
   regression pin that the happy path is unchanged.
5. `short_circuits_on_transport_error_accessor_reflects_state` —
   the public accessor mirrors the configured flag.

The Phase 21 `CountingAuth` mock is extended with a "return
configured `Result` per call" variant so tests can construct
specific error chains.

## B. Python facade

### B.1 — `PyChainedAuth.with_short_circuit_on_transport_error`

[`crates/tako-py/src/py_compat.rs`](crates/tako-py/src/py_compat.rs):

```rust
/// Phase 26.B — opt in to fail-fast on transport errors.
/// Returns a NEW `ChainedAuth`. Idempotent.
fn with_short_circuit_on_transport_error(&self) -> Self {
    let cloned: tako_compat::ChainedAuthResolver = (*self.inner).clone();
    let next = cloned.with_short_circuit_on_transport_error();
    Self { inner: Arc::new(next) }
}
```

### B.2 — `__len__` is unchanged

The `__len__` already reflects child count; the short-circuit
flag is a separate piece of state. No additional Python-side
introspection needed.

### B.3 — Module docstring update

[`python/tako/compat.py`](python/tako/compat.py): mention the
new builder.

### B.4 — Tests

[`tests/python/test_phase26_chained_short_circuit.py`](tests/python/test_phase26_chained_short_circuit.py):

1. `test_with_short_circuit_on_transport_error_returns_new_instance` —
   immutable-builder smoke; the returned `ChainedAuth` is fresh,
   the original unchanged.
2. `test_with_short_circuit_on_transport_error_preserves_children` —
   the flag flip doesn't reset the child list.
3. `test_phase26_aliases_documented_in_module_docstring` —
   module docstring smoke.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  short-circuit tests in 26.A.3 pass; existing Phase 21
  ChainedAuth tests still byte-for-byte green (regression: the
  default behaviour must NOT change).
- `pytest -q tests/python/test_phase26_chained_short_circuit.py`
  — green.

## Out of scope (Phase 27+)

- **Broader infrastructure-error short-circuit** — Phase 26
  short-circuits only on `TakoError::Transport`. Other
  infrastructure-ish variants (`TakoError::RateLimited`,
  `CircuitOpen`, `BudgetExhausted`, the `Provider` source-error
  case) would also benefit but warrant case-by-case analysis.
  If patterns emerge, Phase 27+ may add
  `with_short_circuit_on_infrastructure_errors`.
- **Per-child overrides** — operators may want different
  short-circuit behaviour per child (e.g. "OIDC short-circuits
  but Vault doesn't"). Not yet asked for.
- OIDC mTLS end-to-end integration test, OIDC mTLS cert
  rotation, URL-source for Bedrock / Ollama, Vertex File API
  upload, eval-harness real graders, OIDC refresh-token /
  revocation flows.

## Commits

1. `feat(tako-compat): ChainedAuthResolver fail-fast on transport errors (Phase 26.A)`
2. `feat(tako-py): ChainedAuth short-circuit-on-transport-error facade (Phase 26.B)`
3. `docs: Phase 26 PLAN/README/CHANGELOG flip (v0.27.0)`
