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

Trait surface in `tako-core` is designed so each phase is purely
additive — public APIs from earlier phases never break.

## Roadmap

### Phase 8 candidates (indicative, not yet committed)

- **AB-MCTS native streaming** — interleave rollouts across branches;
  potentially adds a verifier-score event to `OrchEvent`.
- **Rekor checkpoint (`SignedNote`) verification** — out-of-band
  signature over the tree head, separate from the inclusion proof
  itself.
- **`OrchEvent::Recursion` variant** — expose recursion depth +
  confidence on the wire (only if a concrete consumer asks).
- **Streaming-aware `ConfidenceGuard`** — early-abort during a long
  inner generation rather than evaluating buffered final text.

### Beyond (speculative)

- Cosign protobuf-bundle deeper integration (CLI-friendly file inputs;
  full `sigstore-protobuf-specs` migration vs. vendored subset).
- Provider breadth: more open-weight providers, hardware-accel inference
  endpoints.
- Tracing + cost rollup against multi-tenant deployments.
- Eval-driven router fine-tuning loop (Trinity training-from-traces).

## How to work this index

When opening a new phase: pick the next `Phase N` slot, create
`PLAN_PHASE<N>.md` (mirror the structure of [PLAN_PHASE6.md](PLAN_PHASE6.md)
or [PLAN_PHASE7.md](PLAN_PHASE7.md)), add a row to the table above, and
move "in progress" to that row. When the phase ships, flip the status
to "done (date)" and update the corresponding `CHANGELOG.md` anchor.
