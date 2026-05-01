# PLAN — Phase 21 (Composite AuthResolver)

## Context

Phase 20 (v0.21.0, 2026-05-01) finished the vision-content sweep —
all six shipped provider adapters now handle outbound
`ContentPart::Image`. [`PLAN.md`](PLAN.md) lines 49–63 list the
remaining Phase-21 candidates: URL-source images (needs a security
story for server-side fetch), eval-harness real graders (needs a
sandboxed runner), OIDC mTLS (needs workspace-level reqwest TLS
feature changes), OIDC refresh-token / revocation flows (different
model — token consumer rather than validator), and composite
`AuthResolver`s.

Of these, **composite `AuthResolver`** is the cleanest tight
scope. It addresses a real operator gap on the `tako-compat`
OpenAI-compat HTTP server: today an operator picks a single
`auth=` resolver (StaticTokens for dev, JwtAuth / OidcAuth /
VaultAuth for production). Several common patterns require
composing two:

- **"Accept either OIDC bearer OR API key"** — most common
  production deployment when migrating from a static API-key
  scheme to OIDC. Operators currently have to fork the request
  pipeline.
- **"Try OIDC first, fall back to JWT"** — for hybrid
  deployments where some clients are on an OIDC issuer and
  others use a long-lived signed JWT.
- **"Vault for service tokens, OIDC for user tokens"** —
  operators with both deployment models.

`ChainedAuthResolver` ships an `AuthResolver` impl that wraps N
children and tries them in order. The first child to return Ok
short-circuits; on all-Err it returns the last child's error.

Strictly additive — no public-API changes shape; the trait is
unchanged.

mTLS, URL-source images, refresh-token flows, and eval-harness
graders remain deferred to Phase 22+.

**Theme:** *Compose existing `AuthResolver` impls for the common
"accept either of two methods" operator pattern.*

**Tag:** v0.22.0.

## A. `ChainedAuthResolver` in tako-compat

### A.1 — Public API

[`crates/tako-compat/src/auth/chained.rs`](crates/tako-compat/src/auth/chained.rs)
(new file):

```rust
#[derive(Clone, Debug, Default)]
pub struct ChainedAuthResolver {
    children: Vec<Arc<dyn AuthResolver>>,
}

impl ChainedAuthResolver {
    /// Empty chain. `resolve` returns
    /// `TakoError::Invalid("chained auth: no resolvers
    /// configured")` until at least one child is added.
    pub fn new() -> Self { Self::default() }

    /// Append a child resolver. Children are tried in append
    /// order; the first to return `Ok` short-circuits.
    pub fn with(mut self, child: Arc<dyn AuthResolver>) -> Self {
        self.children.push(child);
        self
    }

    /// Number of children. Useful for assertions in test code.
    pub fn len(&self) -> usize { self.children.len() }

    pub fn is_empty(&self) -> bool { self.children.is_empty() }
}
```

`#[derive(Clone)]` matches the cadence of existing resolvers
(`OidcAuthResolver`, `VaultAuthResolver`); the `Arc<dyn ...>`
children are cheap to clone. No feature gate — `ChainedAuthResolver`
is always available because the `AuthResolver` trait is always
available; the children themselves bring whatever feature gates
they were built under.

### A.2 — `resolve()` semantics

```rust
#[async_trait]
impl AuthResolver for ChainedAuthResolver {
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
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.expect("non-empty chain must produce an error on all-fail"))
    }
}
```

Semantics: **any** `Err` from a child falls through to the next.
This matches the common "try OIDC, fall back to API key" pattern
where transient failures in OIDC (network down, JWKS refresh
mid-request) shouldn't strand a static-API-key client. If an
operator wants different semantics ("fail fast on transport
errors"), they can wrap their own resolver. Phase 22+ may add
`with_short_circuit_on_transport_error` if patterns emerge.

### A.3 — Re-export from `auth/mod.rs`

[`crates/tako-compat/src/auth/mod.rs`](crates/tako-compat/src/auth/mod.rs):
add `mod chained;` and `pub use chained::ChainedAuthResolver;`.
No feature gate.

### A.4 — Tests

Seven new unit tests in
[`crates/tako-compat/src/auth/chained.rs`](crates/tako-compat/src/auth/chained.rs):

1. `chained_empty_returns_invalid` — `ChainedAuthResolver::new()`
   without children → `TakoError::Invalid("chained auth: no
   resolvers configured")`.
2. `chained_single_pass_through` — one child, returns its result.
3. `chained_first_match_short_circuits` — verifies second child
   is **not** called when first returns `Ok` (use a counting
   mock).
4. `chained_falls_through_to_second_when_first_errors` —
   first errors, second succeeds → second's principal returned.
5. `chained_returns_last_error_when_all_fail` — error chain
   semantics.
6. `chained_can_nest` — `ChainedAuthResolver` containing another
   `ChainedAuthResolver` works (recursive composition).
7. `chained_is_send_sync_clone_debug` — public-bounds smoke.

A small `CountingAuth(AtomicUsize, Result<Principal, TakoError>)`
mock in the same test module supports the short-circuit
assertion. The struct uses `tako_core::Principal` directly; no
external test-harness dep.

## B. Python facade

### B.1 — `PyChainedAuth` pyclass

[`crates/tako-py/src/py_compat.rs`](crates/tako-py/src/py_compat.rs):

```rust
#[pyclass(name = "ChainedAuth", module = "tako._native")]
pub struct PyChainedAuth {
    inner: Arc<tako_compat::ChainedAuthResolver>,
}

#[pymethods]
impl PyChainedAuth {
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(tako_compat::ChainedAuthResolver::new()),
        }
    }

    /// Append a child resolver. Returns a NEW `ChainedAuth`
    /// (immutable builder; matches the OidcAuth / VaultAuth
    /// cadence). Accepts any `JwtAuth`, `OidcAuth`, `VaultAuth`,
    /// or `ChainedAuth` (recursive).
    fn with(&self, py: Python<'_>, child: Py<PyAny>) -> PyResult<Self> {
        let child = extract_auth_resolver(py, &child)?;
        let cloned: tako_compat::ChainedAuthResolver = (*self.inner).clone();
        let next = cloned.with(child);
        Ok(Self { inner: Arc::new(next) })
    }

    fn __len__(&self) -> usize { self.inner.len() }
}
```

No feature gate — `ChainedAuth` is always available. Each child's
type IS feature-gated, so a wheel built without `auth-oidc`
simply can't construct an `OidcAuth` to add to a chain.

### B.2 — Extend `extract_auth_resolver`

The existing `extract_auth_resolver` helper at
[`crates/tako-py/src/py_compat.rs:119-139`](crates/tako-py/src/py_compat.rs#L119-L139)
gets a fourth `cast` arm for `PyChainedAuth`. Always-on (no
feature gate). Position: at the end of the existing arms so
`Chain<Jwt, ...>` doesn't shadow the JWT cast.

### B.3 — `tako.compat` re-export

[`python/tako/compat.py`](python/tako/compat.py): add
`ChainedAuth = getattr(_native, "ChainedAuth", None)` and append
to `__all__`. Update the module docstring.

### B.4 — Tests

[`tests/python/test_phase21_chained_auth.py`](tests/python/test_phase21_chained_auth.py):

1. `test_chained_auth_attribute_exists` — facade smoke.
2. `test_chained_auth_constructs_empty` — `ChainedAuth()` works.
3. `test_chained_auth_with_child_returns_new_instance` —
   immutable-builder.
4. `test_chained_auth_len_reflects_children` — `__len__` smoke
   with stacked `with` calls.
5. `test_chained_auth_rejects_garbage_child` — `with()` raises
   `ValueError` on a non-`AuthResolver` child.

The Rust unit tests in `chained.rs` remain the source of truth
for behaviour.

## Acceptance criteria (all green)

- `cargo fmt --all` clean.
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean.
- `cargo test --workspace --all-features` — all green; the new
  `chained_*` tests in 21.A.4 pass.
- `pytest -q tests/python/test_phase21_chained_auth.py` — green
  on a wheel built with `--features auth-oidc auth-jwt`.

## Out of scope (Phase 22+)

- mTLS introspection auth methods — needs reqwest TLS feature
  changes at workspace scope.
- URL-source images — needs a `tako-core::ContentPart` extension
  (`ImageUrl` variant or similar) and a security story for
  server-side URL fetch.
- OIDC refresh-token / revocation-endpoint flows — different
  model (tako as token *consumer* rather than validator).
- Eval harness real graders (SWE-Bench Lite, GPQA Diamond).
- OTel end-to-end real-collector test.
- Replace `TODO(<org>)` repository URL placeholders at first
  public-org publish.
- "Fail fast on transport errors" semantics for
  `ChainedAuthResolver` — if usage patterns emerge, Phase 22+
  may add `with_short_circuit_on_transport_error`.

## Commits

1. `feat(tako-compat): ChainedAuthResolver composite auth (Phase 21.A)`
2. `feat(tako-py): ChainedAuth Python facade (Phase 21.B)`
3. `docs: Phase 21 PLAN/README/CHANGELOG flip (v0.22.0)`
