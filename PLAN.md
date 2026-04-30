# PLAN — rolling project index

> Per spec §19 rule 1: this is the rolling project plan that future
> Claude Code sessions read on entry. **Each phase owns its own
> `PLAN_PHASE*.md`**; this file is the high-level index + roadmap.
>
> Workflow rules (commit cadence, fmt/clippy/test gates, etc.) live in
> [CLAUDE.md](CLAUDE.md). Architectural rules live in
> [ARCHITECTURE.md](ARCHITECTURE.md).

`tako` is a Rust-core, Python-facade framework for enterprise agentic
systems. The Rust workspace lives under `crates/`, the Python facade
under `python/tako/`, and the wheel target is `crates/tako-py` built
with maturin + PyO3. See [README.md](README.md) for the project
synopsis and quickstart.

## Phase index

| Phase | Version | Status | Plan doc | Changelog |
|-------|---------|--------|----------|-----------|
| 1 — Foundation | v0.1.0 | done (2026-04-28) | [PLAN_PHASE1.md](PLAN_PHASE1.md) | [`## [0.1.0]`](CHANGELOG.md) |
| 2 — Orchestration (+ bundled 1.5) | v0.2.0 | done (2026-04-29) | [PLAN_PHASE2.md](PLAN_PHASE2.md) | [`## [0.2.0]`](CHANGELOG.md) |
| 2.5 — Cloud breadth | v0.3.0 | done (2026-04-29) | [PLAN_PHASE25.md](PLAN_PHASE25.md) | [`## [0.3.0]`](CHANGELOG.md) |
| 3 — Learned coordination | v0.4.0 | done (2026-04-29) | [PLAN_PHASE3.md](PLAN_PHASE3.md) | [`## [0.4.0]`](CHANGELOG.md) |
| 4 — Search & scale | v0.5.0 | done (2026-04-29, retro plan) | [PLAN_PHASE4.md](PLAN_PHASE4.md) | [`## [0.5.0]`](CHANGELOG.md) |
| 5 — Production hardening | v0.6.0 | done (2026-04-29) | [PLAN_PHASE5.md](PLAN_PHASE5.md) | [`## [0.6.0]`](CHANGELOG.md) |
| 6 — Production hardening, continued | v0.7.0 | done (2026-04-29) | [PLAN_PHASE6.md](PLAN_PHASE6.md) | [`## [0.7.0]`](CHANGELOG.md) |
| 7 — Sigstore + streaming closures | v0.8.0 | done (2026-04-29) | [PLAN_PHASE7.md](PLAN_PHASE7.md) | [`## [0.8.0]`](CHANGELOG.md) |
| 8 — Search streaming + transparency-log completeness | v0.9.0 | done (2026-04-29) | [PLAN_PHASE8.md](PLAN_PHASE8.md) | [`## [0.9.0]`](CHANGELOG.md) |
| 9 — Cost-aware streaming guards + log freshness + protocol completeness + router-driven AB-MCTS | v0.10.0 | done (2026-04-30) | [PLAN_PHASE9.md](PLAN_PHASE9.md) | [`## [0.10.0]`](CHANGELOG.md) |
| 10 — Phase 9 follow-on completeness + cross-orchestrator verifier scores + Python provider streaming | v0.11.0 | done (2026-04-30) | [PLAN_PHASE10.md](PLAN_PHASE10.md) | [`## [0.11.0]`](CHANGELOG.md) |
| 11 — Sigstore security hardening + http-generic provider streaming | v0.12.0 | done (2026-04-30) | [PLAN_PHASE11.md](PLAN_PHASE11.md) | [`## [0.12.0]`](CHANGELOG.md) |

Trait surface in `tako-core` is designed so each phase is purely
additive — public APIs from earlier phases never break.

## Roadmap

### Phase 12 candidates (indicative, not yet committed)

- **MCP Streamable HTTP — SSE upgrade + `Mcp-Session-Id` lifecycle.**
  Promised in Phase 2; transport still yields an empty stream.
  [crates/tako-mcp/src/transport/streamable_http.rs:154](crates/tako-mcp/src/transport/streamable_http.rs#L154).
- **Vision / image content support across providers.** Anthropic,
  Vertex, and Bedrock all have stub markers; multi-crate
  cross-cutting effort that warrants a focused phase.
- **Eval harness real graders** (SWE-Bench Lite, GPQA Diamond) —
  promised in Phase 3 PLAN, still raise `NotImplementedError`.
  Sandboxed runner needed.
- **Redis-backed `StateStore`** — sibling to Phase 10.A's
  `JsonStateStore` for multi-replica deployments where multiple
  workers consume the same Rekor freshness anchor. Requires
  introducing a `StateStore` trait first.
- **Streaming-aware verifier in Trinity / Conductor.** Phase 10.C
  emits `VerifierScore` only at synthesis-complete boundaries.
  Per-delta verifier calls would need the same opt-in cost-control
  surface as `LlmJudgeGuard::with_streaming_min_chars`. Lands when
  a concrete consumer asks.
- **Python facade for `HttpGenericProvider`.** Phase 11.B added the
  Rust streaming surface; the Python facade was planned but
  skipped because no `tako.providers.HttpGeneric` class exists
  today (it is configured via Rust code or community-supplied
  wrappers). Adding the full PyO3 binding is a Phase 12 candidate
  if community demand appears.
- **`tako-compat` real auth providers** — Vault / JWT / OIDC,
  beyond `StaticTokens` ([crates/tako-compat/src/auth.rs:5](crates/tako-compat/src/auth.rs#L5)).

### Beyond (speculative)

- Cosign protobuf-bundle deeper integration (CLI-friendly file inputs;
  full `sigstore-protobuf-specs` migration vs. vendored subset).
- Provider breadth: more open-weight providers, hardware-accel inference
  endpoints.
- Tracing + cost rollup against multi-tenant deployments.
- Eval-driven router fine-tuning loop (Trinity training-from-traces).

### Backlog (uncommitted)

Items surfaced from a 2026-04-30 audit of phase markers across the codebase.
Not yet slotted into a phase; recorded here so they don't get lost between
phase transitions. File/line references point at the stale marker, not at
where the fix would land.

#### Stale phase markers — promised but not delivered

- [ ] **MCP Streamable HTTP — SSE upgrade + `Mcp-Session-Id` lifecycle.**
  Comment promises Phase 2; transport still yields an empty stream.
  [crates/tako-mcp/src/transport/streamable_http.rs:2-3](crates/tako-mcp/src/transport/streamable_http.rs#L2-L3),
  [:154](crates/tako-mcp/src/transport/streamable_http.rs#L154).
- [x] **`tako-providers/http-generic` streaming.** Closed in Phase
  11.B (v0.12.0): set `HttpGenericConfig::stream_config` to a
  `StreamConfig::OpenAiSse` or `StreamConfig::NdJson` variant
  with JSON-pointer-based delta extraction.
- [x] **Python custom provider streaming.** Closed in Phase 10.D
  (v0.11.0): pass `stream=async_gen_fn` to
  `tako.providers.PythonProvider` and the Rust side iterates the
  async generator via `__anext__()`, deserialising each yielded
  dict to a `ChatChunk` via the `kind`-tagged JSON shape.
- [ ] **`tako-compat` real auth providers** — Vault / JWT / OIDC.
  Only `StaticTokens` ships.
  [crates/tako-compat/src/auth.rs:5](crates/tako-compat/src/auth.rs#L5).
- [ ] **Vision / image content support across providers.**
  Anthropic ([convert.rs:171](crates/tako-providers/anthropic/src/convert.rs#L171)),
  Vertex ([convert.rs:203](crates/tako-providers/vertex/src/convert.rs#L203)),
  Bedrock ([convert.rs:268](crates/tako-providers/bedrock/src/convert.rs#L268)).
- [ ] **Eval harness real graders.** `swe_bench_lite` and `gpqa_diamond`
  raise `NotImplementedError`; real SWE-Bench (apply patch + run sandboxed
  repo tests) deferred to "a later phase".
  [python/tako/eval/harness.py:9-10](python/tako/eval/harness.py#L9-L10),
  [python/tako/eval/datasets/external.py:8-11](python/tako/eval/datasets/external.py#L8-L11).
- [ ] **OTel end-to-end test against a real gRPC collector.** Full e2e
  test deferred from Phase 1.5 acceptance criteria.
  [tests/python/test_otlp.py:13-16](tests/python/test_otlp.py#L13-L16).
- [ ] **Vertex deterministic-per-call placeholder logic.** Stub flagged
  inline; revisit when usage patterns warrant.
  [crates/tako-providers/vertex/src/convert.rs:291](crates/tako-providers/vertex/src/convert.rs#L291).

#### Documentation maintenance

- [x] **Bring `README.md` feature matrix current.** Phase 9.E
  swept the matrix through Phase 9; Phase 10.E added a Phase 10
  column (verifier scores in Trinity / Conductor; tool-call
  lifecycle named SSE events; on-disk JsonStateStore; Python
  custom provider streaming). Roadmap section enumerates Phases
  1–10.

#### First-publish placeholders

- [ ] **Replace `TODO(<org>)` repository URLs** at first public-org
  publish (intentional single-point-of-edit per `PLAN_PHASE1.md` line 2).
  [Cargo.toml:26](Cargo.toml#L26), [README.md](README.md),
  [CONTRIBUTING.md](CONTRIBUTING.md),
  [CHANGELOG.md:999-1008](CHANGELOG.md#L999-L1008),
  [crates/tako-core/src/lib.rs:8-9](crates/tako-core/src/lib.rs#L8-L9).
- [ ] **Resolve `TODO(community)` placeholders** — Discussions
  categories, Discord/Matrix room ([README.md:135-136](README.md#L135-L136)),
  conduct@ contact (CODE_OF_CONDUCT.md), security@ contact
  (SECURITY.md).

## How to work this index

When opening a new phase: pick the next `Phase N` slot, create
`PLAN_PHASE<N>.md` (mirror the structure of [PLAN_PHASE6.md](PLAN_PHASE6.md)
or [PLAN_PHASE7.md](PLAN_PHASE7.md)), add a row to the table above, and
move "in progress" to that row. When the phase ships, flip the status
to "done (date)" and update the corresponding `CHANGELOG.md` anchor.
