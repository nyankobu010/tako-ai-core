# PLAN — Phase 36 (Per-child `ChainedAuthResolver` short-circuit policy override)

> **Status: in progress.** Targets v0.37.0. Stacks on top of
> Phase 35 (v0.36.0); rebases onto main once
> [tako-ai-core#33](https://github.com/nyankobu010/tako-ai-core/pull/33)
> merges. Carry-forward from
> [PLAN.md → Phase 36 candidates](../PLAN.md).

## Context

Phase 21.A introduced [`ChainedAuthResolver`](../crates/tako-compat/src/auth/chained.rs) — a composite [`AuthResolver`] that tries N child resolvers in append order and returns the first `Ok`. Phase 21 default semantics: every `Err` falls through to the next child.

Phase 26 added `with_short_circuit_on_transport_error()` — opt-in fail-fast when any child returns `TakoError::Transport` (the "OIDC issuer unreachable" case where falling through to a static-token resolver would mask the actionable diagnostic).

Phase 27 widened the policy to `with_short_circuit_on_infrastructure_errors()` — fail-fast on `Transport`, `RateLimited`, `CircuitOpen`, `BudgetExhausted`. Auth-decision errors (`Invalid` / `PolicyDenied`) and vendor errors (`Provider`) continue to fall through.

Both Phase 26 and Phase 27 set the policy **chain-wide**. That's the right default for most operators, but it forces a single sensitivity level on the whole chain. Real deployments often mix:

- A **critical** primary backend (OIDC issuer) where transport / rate-limit / circuit failures should halt the chain — operators want the actionable error, not a misleading "unknown bearer" from a fallback.
- A **graceful tail** fallback (in-process static API keys, Vault) that should continue to fall through even on infra-style errors — if the fallback is in-process or has its own circuit, there's no risk of masking, and operators want the "tail child" to keep serving.

Today the chain-wide flags conflate the two. An operator setting `with_short_circuit_on_infrastructure_errors()` to protect the OIDC backend also forces the static-tokens tail to halt on any of the four infra variants — even though a static-token resolver functionally cannot raise `RateLimited` / `CircuitOpen` / `BudgetExhausted` and `Transport` from it would be a bug, not the "real" diagnostic the chain-wide flag is designed to surface.

Phase 36 adds per-child policy override so operators can mark individual children as:

- **Inherit** (default) — chain-wide policy applies, byte-for-byte Phase 21 / 26 / 27 semantics.
- **Always fall through** — the chain-wide policy is overridden for this child; every `Err` falls through. Useful for graceful-tail fallbacks.
- **Transport-only short-circuit** — narrower than chain-wide infrastructure: even when the chain is configured with `with_short_circuit_on_infrastructure_errors`, this child only halts on `Transport`.
- **Infrastructure short-circuit** — broader than chain-wide transport: even when the chain is configured with the narrower Phase 26 flag, this child halts on the full Phase 27 set.

## Why now

Phase 27 closed the chain-wide policy ladder; the natural follow-on is per-child override. The internal data structure already routes every error through a single `match` in `resolve()`, so the change is mechanical: replace `Vec<Arc<dyn AuthResolver>>` with `Vec<(Arc<dyn AuthResolver>, ChildShortCircuitPolicy)>` and consult the per-child policy when it's not `Inherit`.

This phase does not unblock anything else — it's a scoped operator-knob enhancement. It's a good "out of mTLS land" rotation after Phases 33 / 35 (mTLS-rotation themes).

## Scope summary

| Section | What | Files |
|---------|------|-------|
| 36.A | `ChildShortCircuitPolicy` enum + `then_with_short_circuit` builder + per-child override logic | [`crates/tako-compat/src/auth/chained.rs`](../crates/tako-compat/src/auth/chained.rs), [`crates/tako-compat/src/auth/mod.rs`](../crates/tako-compat/src/auth/mod.rs), [`crates/tako-compat/src/lib.rs`](../crates/tako-compat/src/lib.rs) |
| 36.B | Python facade `ChainedAuth.then_with_short_circuit(child, policy)` | [`crates/tako-py/src/py_compat.rs`](../crates/tako-py/src/py_compat.rs), [`python/tako/compat.py`](../python/tako/compat.py) |
| 36.C | Recipe doc — per-child override example | [`docs/recipes/chained_auth.md`](../docs/recipes/chained_auth.md) |
| 36.D | Workspace + Python version 0.36.0 → 0.37.0 | [`Cargo.toml`](../Cargo.toml), [`pyproject.toml`](../pyproject.toml), [`python/tako/__init__.py`](../python/tako/__init__.py), [`tests/python/test_smoke.py`](../tests/python/test_smoke.py) |
| 36.E | PLAN.md row + Phase 37 candidate-list refresh | [`PLAN.md`](../PLAN.md) |
| 36.F | CHANGELOG.md `[0.37.0]` entry | [`CHANGELOG.md`](../CHANGELOG.md) |
| 36.G | Python smoke test | [`tests/python/test_phase36_chained_per_child_policy.py`](../tests/python/test_phase36_chained_per_child_policy.py) |

## What this phase will land

### 36.A — Rust core: `ChildShortCircuitPolicy` + per-child override

Public enum:

```rust
/// Phase 36 — per-child override for the chain-wide
/// short-circuit policy set by
/// [`ChainedAuthResolver::with_short_circuit_on_transport_error`]
/// (Phase 26) and
/// [`ChainedAuthResolver::with_short_circuit_on_infrastructure_errors`]
/// (Phase 27).
///
/// Default [`Self::Inherit`] preserves Phase 21 / 26 / 27
/// chain-wide semantics byte-for-byte: the per-child override
/// is inert unless explicitly set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ChildShortCircuitPolicy {
    /// Inherit the chain-wide policy. Default.
    #[default]
    Inherit,
    /// Override: every `Err` from this child falls through to
    /// the next, regardless of chain-wide policy. Useful for
    /// graceful-tail fallbacks (static-token last-resort that
    /// must keep serving even when the chain-wide flag is set).
    AlwaysFallThrough,
    /// Override: short-circuit only on `Transport`. Narrower
    /// than chain-wide `AllInfrastructure`.
    TransportOnly,
    /// Override: short-circuit on `Transport` / `RateLimited` /
    /// `CircuitOpen` / `BudgetExhausted`. Broader than
    /// chain-wide `TransportOnly`.
    AllInfrastructure,
}
```

New builder method:

```rust
impl ChainedAuthResolver {
    /// Phase 36 — append a child WITH a per-child
    /// short-circuit-policy override. The chain-wide policy
    /// (set by Phase 26 / 27 builders) still applies to every
    /// child whose own override is `ChildShortCircuitPolicy::Inherit`
    /// (the [`Self::then`] default).
    pub fn then_with_short_circuit(
        mut self,
        child: Arc<dyn AuthResolver>,
        policy: ChildShortCircuitPolicy,
    ) -> Self;
}
```

Internal data structure change: `children: Vec<Arc<dyn AuthResolver>>` becomes `Vec<ChildEntry>` where:

```rust
struct ChildEntry {
    resolver: Arc<dyn AuthResolver>,
    policy: ChildShortCircuitPolicy,
}
```

`then` becomes a thin wrapper that pushes with `Inherit`. The `len`, `is_empty`, `Clone`, `Debug` behaviours are preserved.

The `resolve()` match-on-policy gains an outer "if per-child override is non-Inherit, use it; else fall back to chain-wide policy" layer.

#### Public re-exports

`ChildShortCircuitPolicy` is re-exported from `tako_compat::auth::*` and from the crate root via `crates/tako-compat/src/lib.rs`.

#### Tests

In the existing `chained::tests` module, add ~6 tests:

1. `per_child_always_fall_through_overrides_chain_wide` — chain-wide infra short-circuit, but a `RateLimited`-emitting child marked `AlwaysFallThrough` falls through; subsequent child sees the call.
2. `per_child_transport_only_overrides_chain_wide_infrastructure` — chain-wide infra, child marked `TransportOnly` falls through on `RateLimited` but halts on `Transport`.
3. `per_child_all_infrastructure_overrides_chain_wide_transport_only` — chain-wide `TransportOnly`, child marked `AllInfrastructure` halts on `RateLimited`.
4. `per_child_inherit_default_preserves_chain_wide` — explicit `Inherit` matches `then(...)` byte-for-byte.
5. `per_child_policy_does_not_affect_happy_path` — an `Ok` child still short-circuits the whole chain regardless of per-child policy.
6. `then_and_then_with_short_circuit_can_mix` — an operator can mix bare `then` (Inherit) and `then_with_short_circuit` (override) children in any order.

### 36.B — Python facade

Add to `PyChainedAuth`:

```rust
#[pymethods]
impl PyChainedAuth {
    /// Phase 36 — append a child with a per-child
    /// short-circuit override. `policy` accepts the
    /// case-insensitive aliases:
    ///
    /// - `"inherit"` (default — same as `then(child)`)
    /// - `"always_fall_through"` / `"always-fall-through"`
    /// - `"transport_only"` / `"transport-only"`
    /// - `"all_infrastructure"` / `"all-infrastructure"`
    fn then_with_short_circuit(
        &self,
        py: Python<'_>,
        child: Py<PyAny>,
        policy: &str,
    ) -> PyResult<Self>;
}
```

Unrecognized policy strings raise `ValueError` listing the accepted aliases.

### 36.C — Recipe doc

[`docs/recipes/chained_auth.md`](../docs/recipes/chained_auth.md) gets a "Per-child policy override" section with an end-to-end example: a 3-child chain (OIDC critical → JWT critical → static dev tail) using chain-wide infra short-circuit + per-child `AlwaysFallThrough` on the static tail.

### 36.D — Version bump

Workspace + Python: `0.36.0` → `0.37.0` across `Cargo.toml` (workspace package + every `path = "..."` workspace dep), `pyproject.toml`, `python/tako/__init__.py`, and `tests/python/test_smoke.py`'s pinned assertion.

### 36.E — PLAN.md update

- New row: `| 36 — Per-child ChainedAuthResolver short-circuit policy | v0.37.0 | done (date) | plans/PLAN_PHASE36.md | [\`## [0.37.0]\`](CHANGELOG.md) |`
- "Phase 37 candidates" section replaces "Phase 36 candidates"; drops the per-child-policy-override entry (now shipped); rest of the carry-forward list stays.

### 36.F — CHANGELOG entry

```markdown
## [0.37.0] - <date>

Phase 36 — per-child `ChainedAuthResolver` short-circuit policy
override. Operators wiring composite auth chains can now mark
individual children as Inherit / AlwaysFallThrough /
TransportOnly / AllInfrastructure independent of the chain-wide
policy. Common pattern: chain-wide
`with_short_circuit_on_infrastructure_errors` plus a final
in-process static-tokens child marked `AlwaysFallThrough` so a
spurious infra error from the tail doesn't strand the chain.

### Added
- `tako-compat`: `ChildShortCircuitPolicy` enum +
  `ChainedAuthResolver::then_with_short_circuit(child, policy)`
  builder. Existing `then(child)` keeps its Phase 21 cadence
  (defaults to `ChildShortCircuitPolicy::Inherit`). Python
  facade: `ChainedAuth.then_with_short_circuit(child, policy)`
  accepting `"inherit"` / `"always_fall_through"` /
  `"transport_only"` / `"all_infrastructure"` (and kebab
  variants).
```

### 36.G — Python smoke test

`tests/python/test_phase36_chained_per_child_policy.py` — three tests:

1. `test_chained_auth_has_then_with_short_circuit` — facade attribute presence.
2. `test_then_with_short_circuit_accepts_known_policies` — happy-path build with each accepted alias.
3. `test_then_with_short_circuit_rejects_unknown_policy` — `ValueError` on a typo.

## Critical files

**Modified:**
- `crates/tako-compat/src/auth/chained.rs` — `ChildShortCircuitPolicy` enum + `then_with_short_circuit` + per-child override in `resolve()` + tests.
- `crates/tako-compat/src/auth/mod.rs` — re-export `ChildShortCircuitPolicy`.
- `crates/tako-compat/src/lib.rs` — re-export `ChildShortCircuitPolicy`.
- `crates/tako-py/src/py_compat.rs` — `PyChainedAuth::then_with_short_circuit`.
- `python/tako/compat.py` — module docstring + `__all__` update.
- `docs/recipes/chained_auth.md` — per-child override section.
- `PLAN.md` / `CHANGELOG.md` / `Cargo.toml` / `pyproject.toml` / `python/tako/__init__.py` / `tests/python/test_smoke.py` — version flip + index update.

**Created:**
- `plans/PLAN_PHASE36.md` (this file).
- `tests/python/test_phase36_chained_per_child_policy.py`.

## Verification

1. `cargo fmt --all -- --check` passes.
2. `cargo clippy -p tako-compat --all-features -- -D warnings` passes.
3. `cargo clippy --workspace --exclude tako-py --all-features -- -D warnings` passes.
4. `cargo test -p tako-compat --all-features` passes — 129 + 6 new = 135 tests.
5. `cargo test --workspace --exclude tako-py --all-features` passes.
6. `ruff format --check` + `ruff check` clean.
7. `pytest -q` passes.
8. `maturin develop` builds and `pytest tests/python/test_phase36_chained_per_child_policy.py` passes.

## Out of scope

- **Per-child Phase 27 `Provider` short-circuit.** The Phase 27 plan flagged that `Provider` short-circuit warrants finer discrimination on the embedded error; that remains deferred regardless of per-child override. A `ChildShortCircuitPolicy::Provider` variant would prematurely commit to a flat-yes/no semantics for vendor errors.
- **Per-child weighting / preference ordering.** The chain stays linear append-order. Re-ordering at runtime is a different feature.
- **Run-time policy mutation.** All policies (chain-wide and per-child) are set at build time. Atomic-swap rotation (à la Phase 33's `MtlsClient`) would require a different internal layout and isn't on any operator's ask list.
