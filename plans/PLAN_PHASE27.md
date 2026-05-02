# PLAN — Phase 27 (ChainedAuthResolver broader infrastructure-error short-circuit)

## Context

Phase 26 (v0.27.0, 2026-05-01) shipped
`ChainedAuthResolver::with_short_circuit_on_transport_error` —
opt-in fail-fast on `TakoError::Transport`. The Phase 26 PLAN
explicitly noted: "Other infrastructure-ish variants
(`RateLimited`, `CircuitOpen`, `BudgetExhausted`, the `Provider`
source-error case) would also benefit but warrant case-by-case
analysis. If patterns emerge, Phase 27+ may add
`with_short_circuit_on_infrastructure_errors`."

Phase 27 ships that broader flag.

The relevant `TakoError` variants:

- `Transport(String)` — network failure. Phase 26 short-circuit
  target. **Short-circuit ✓**
- `RateLimited(Duration)` — operator should respect rate limit.
  Falling through to a different resolver doesn't reset the
  limit on the upstream issuer. **Short-circuit ✓**
- `CircuitOpen` — internal failsafe circuit broke (e.g. JWKS
  fetch threshold). Falling through doesn't reset. **Short-circuit ✓**
- `BudgetExhausted(String)` — operator-set spend cap reached.
  Falling through circumvents the cap. **Short-circuit ✓**
- `Provider { source, ... }` — vendor API error. Could be auth
  failure (fall through) OR vendor rate limit (short-circuit).
  Without finer discrimination on the embedded error, the safe
  default is fall-through. **Fall through (deferred)**
- `Tool(String)` — tool execution error. Not relevant for
  `AuthResolver::resolve`; never reaches here in practice.
- `Invalid(String)` — auth decision (token bad, signature
  mismatch). Next resolver might overturn. **Fall through**
- `PolicyDenied(String)` — policy decision. Next resolver might
  overturn. **Fall through**

So `with_short_circuit_on_infrastructure_errors` covers the four
"definitely infra" variants:
`Transport` ∪ `RateLimited` ∪ `CircuitOpen` ∪ `BudgetExhausted`.

**Theme:** *Extend Phase 26 to all error variants where falling
through would mask a non-auth-decision failure or circumvent an
operator-set guard.*

**Tag:** v0.28.0.

## A. `ChainedAuthResolver::with_short_circuit_on_infrastructure_errors`

### A.1 — Refactor flag → policy enum

Phase 26 stored `short_circuit_on_transport_error: bool`.
Phase 27 needs a tri-state: `None` / `TransportOnly` /
`AllInfrastructure`. Refactor the private field to a
`ShortCircuitPolicy` enum.

The field is private; the public boolean accessors stay. No
public-API churn; existing Phase 26 callers
(`with_short_circuit_on_transport_error()` +
`short_circuits_on_transport_error()`) work byte-for-byte.

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum ShortCircuitPolicy {
    /// Phase 21 default — fall through on every `Err`.
    #[default]
    None,
    /// Phase 26 — short-circuit only on `TakoError::Transport`.
    TransportOnly,
    /// Phase 27 — short-circuit on `Transport`, `RateLimited`,
    /// `CircuitOpen`, and `BudgetExhausted`. `Provider`,
    /// `Invalid`, `PolicyDenied` still fall through.
    AllInfrastructure,
}

#[derive(Clone, Debug, Default)]
pub struct ChainedAuthResolver {
    children: Vec<Arc<dyn AuthResolver>>,
    short_circuit_policy: ShortCircuitPolicy,
}
```

### A.2 — Public surface

```rust
impl ChainedAuthResolver {
    /// Phase 26 — short-circuit on `Transport` only. Last-write
    /// wins between the Phase-26 / Phase-27 builders; the policy
    /// is overwritten, not merged.
    pub fn with_short_circuit_on_transport_error(mut self) -> Self {
        self.short_circuit_policy = ShortCircuitPolicy::TransportOnly;
        self
    }

    /// Phase 27 — short-circuit on infrastructure errors:
    /// `Transport`, `RateLimited`, `CircuitOpen`,
    /// `BudgetExhausted`. Auth-decision errors (`Invalid`,
    /// `PolicyDenied`) and vendor errors (`Provider`) still fall
    /// through.
    ///
    /// Useful when `RateLimited` / `CircuitOpen` /
    /// `BudgetExhausted` from one resolver shouldn't be masked by
    /// fall-through to another (each represents either an
    /// infrastructure failure or an operator-set guard that
    /// falling through would circumvent).
    pub fn with_short_circuit_on_infrastructure_errors(mut self) -> Self {
        self.short_circuit_policy = ShortCircuitPolicy::AllInfrastructure;
        self
    }

    /// Phase 26 — accessor: returns `true` for both
    /// `TransportOnly` and `AllInfrastructure` policies (both
    /// short-circuit on `Transport`).
    pub fn short_circuits_on_transport_error(&self) -> bool {
        !matches!(self.short_circuit_policy, ShortCircuitPolicy::None)
    }

    /// Phase 27 — accessor for the broader policy.
    pub fn short_circuits_on_infrastructure_errors(&self) -> bool {
        matches!(self.short_circuit_policy, ShortCircuitPolicy::AllInfrastructure)
    }
}
```

### A.3 — `resolve()` extension

```rust
let should_short_circuit = match self.short_circuit_policy {
    ShortCircuitPolicy::None => false,
    ShortCircuitPolicy::TransportOnly => matches!(e, TakoError::Transport(_)),
    ShortCircuitPolicy::AllInfrastructure => matches!(
        e,
        TakoError::Transport(_)
            | TakoError::RateLimited(_)
            | TakoError::CircuitOpen
            | TakoError::BudgetExhausted(_)
    ),
};
if should_short_circuit {
    return Err(e);
}
```

### A.4 — Tests

Six new unit tests in
[`crates/tako-compat/src/auth/chained.rs`](crates/tako-compat/src/auth/chained.rs):

1. `infrastructure_short_circuit_default_falls_through_on_rate_limited` —
   regression: without `with_short_circuit_on_infrastructure_errors`,
   `RateLimited` falls through.
2. `infrastructure_short_circuit_returns_immediately_on_rate_limited` —
   `RateLimited` propagates with broader flag set.
3. `infrastructure_short_circuit_returns_immediately_on_circuit_open` —
   `CircuitOpen` propagates.
4. `infrastructure_short_circuit_returns_immediately_on_budget_exhausted` —
   `BudgetExhausted` propagates.
5. `infrastructure_short_circuit_falls_through_on_invalid_error` —
   only the four infra variants short-circuit; `Invalid` /
   `PolicyDenied` / `Provider` still fall through (covered by
   one test using `Invalid` as proxy for the auth-decision
   class).
6. `transport_only_falls_through_on_rate_limited_when_transport_only_set` —
   the narrower Phase-26 flag does NOT short-circuit on
   `RateLimited` (regression pin: extending the policy enum
   doesn't accidentally broaden the Phase-26 semantics).

The `CountingAuth` test mock is extended further to preserve
`RateLimited` / `CircuitOpen` / `BudgetExhausted` variants
(currently only `Transport` and `Invalid` round-trip; others
collapse into `Invalid`).

## B. Python facade

### B.1 — `PyChainedAuth.with_short_circuit_on_infrastructure_errors`

[`crates/tako-py/src/py_compat.rs`](crates/tako-py/src/py_compat.rs):
new builder method + accessor mirroring the Rust API.

### B.2 — Module docstring update

[`python/tako/compat.py`](python/tako/compat.py): mention the
new builder + the variant coverage.

### B.3 — Tests

[`tests/python/test_phase27_chained_infrastructure.py`](tests/python/test_phase27_chained_infrastructure.py):
facade attribute presence + immutable-builder smoke +
last-write-wins semantics between the Phase-26 and Phase-27
builders.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  infrastructure-short-circuit tests in 27.A.4 pass; existing
  Phase 21 + 26 ChainedAuth tests still byte-for-byte green
  (regression: the Phase-26 narrower flag must NOT broaden
  scope after the policy enum refactor).
- `pytest -q tests/python/test_phase27_chained_infrastructure.py`
  — green.

## Out of scope (Phase 28+)

- **`TakoError::Provider` short-circuit** — vendor-error
  short-circuit warrants finer discrimination on the embedded
  error (auth failure vs. rate limit vs. internal). Deferred.
- **Per-child policy override** — operators may want different
  short-circuit policies per child (e.g. "OIDC short-circuits
  but Vault doesn't"). Not yet asked for.
- **OIDC mTLS end-to-end integration test, mTLS cert rotation,
  URL-source for Bedrock / Ollama, Vertex File API upload,
  eval-harness real graders, OIDC refresh-token / revocation**
  — all carried over.

## Commits

1. `feat(tako-compat): ChainedAuthResolver infrastructure-error short-circuit (Phase 27.A)`
2. `feat(tako-py): ChainedAuth infrastructure-error short-circuit facade (Phase 27.B)`
3. `docs: Phase 27 PLAN/README/CHANGELOG flip (v0.28.0)`
