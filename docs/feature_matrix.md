# Feature matrix — per-phase ledger

Below is the chronological ledger of which capability landed in which phase.
For a high-level summary of what's available **today**, see the
[Capabilities table on the project home page](index.md#whats-inside-current-release)
and the [README.md root](https://github.com/nyankobu010/tako-ai-core#capabilities-current-release).

> Reading guide: ✅ marks the phase a capability shipped in.
> ➕ marks each subsequent phase that extended it. Empty cells mean
> no change in that phase.

| Capability                         | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 | Phase 6 | Phase 7 | Phase 8 | Phase 9 | Phase 10 | Phase 11 | Phase 12 | Phase 13 | Phase 14 | Phase 15 | Phase 16 | Phase 17 | Phase 18 | Phase 19 | Phase 20 | Phase 21 | Phase 22 | Phase 23 | Phase 24 | Phase 25 | Phase 26 | Phase 27 | Phase 28 | Phase 29 | Phase 30 | Phase 31 | Phase 32 | Phase 33 |
|------------------------------------|:-------:|:-------:|:-------:|:-------:|:-------:|:-------:|:-------:|:-------:|:-------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|:--------:|
| `LlmProvider` trait + adapters     | ✅ Anthropic, OpenAI, http-generic | ➕ Azure, Bedrock, Vertex | | ➕ Mistral, Ollama | | | | | | ➕ Python custom provider streaming | ➕ `http-generic` streaming (`StreamConfig`) | ➕ `tako.providers.HttpGeneric` Python facade | | | | | | | ➕ outbound vision content (`ContentPart::Image`) on Anthropic + OpenAI | ➕ outbound vision content on Vertex (Gemini `inlineData`) + Mistral (OpenAI-compatible `image_url`) + Ollama (sibling `images` field) — completes the six-of-six provider sweep | | ➕ URL-source images (`ContentPart::ImageUrl`) on Anthropic + OpenAI + Mistral; vendor's API server fetches the URL | ➕ URL-source images on Vertex (Gemini `fileData` — accepts `gs://` GCS + `https://` URLs Google fetches) | | | | | ➕ URL-source images on Bedrock + Ollama via opt-in tako-side pre-fetch (`url_prefetch=true`; `https`-only, configurable timeout / size cap, MIME validation) | ➕ default-on private-IP blocklist (loopback / RFC 1918 / link-local / multicast / IPv6 unique-local + link-local + IPv4-mapped) at DNS-resolve time + IP-literal check; DNS-rebinding mitigation via `reqwest::dns::Resolve` impl validating EVERY returned IP; `tako.providers.Ollama` Python facade closes Phase 28.C asymmetry | ➕ per-host allowlist (`with_url_prefetch_allow_host(host)`) — chainable builder lets operators permit specific internal hostnames (e.g., a private artifact registry on a private RFC 1918 address) while keeping the rest of the blocklist active; `url_prefetch_allow_hosts: list[str] \| None` kwarg threaded through `tako.providers.Bedrock` + `tako.providers.Ollama` | ➕ wildcard suffix patterns (`*.internal.corp`) on the per-host allowlist via new `AllowList` struct that splits exact-match from suffix-match at config time; multi-level matching by default (`*.X` matches `a.X` AND `b.a.X`); leftmost-`*` convention only | ➕ CIDR allowlist (`with_url_prefetch_allow_cidr("10.0.5.0/24")`) — IPv4 + IPv6 CIDRs via new `ipnet` workspace dep; bypass triggers when a resolved IP (or IP literal) falls inside any allowlisted subnet; `url_prefetch_allow_cidrs: list[str] \| None` Python kwarg on both providers — closes the operator allowlist arc (exact + wildcard + CIDR forms) |
| OpenAI-compat HTTP server          |         | ✅      |         |         |         |         |         | ➕ `tako.*` SSE extensions (Phase 9) | | ➕ `tako.tool_call_*` named events | | | | ➕ JWT / OIDC / Vault `AuthResolver` impls (cargo features) | ➕ Vault AppRole / Kubernetes token rotation; OIDC RFC 7662 introspection | ➕ Vault Enterprise namespace; OIDC introspection `client_secret_post` auth method | ➕ OIDC introspection RFC 8414 discovery-driven auth-method selection; `client_secret_jwt` (RFC 7521 / 7523) | ➕ OIDC introspection `private_key_jwt` (RFC 7521 / 7523, RS256 / ES256 / EdDSA); end-session endpoint helper (OIDC Session Management 1.0) | | | ➕ `ChainedAuthResolver` composite resolver (try children in order; first `Ok` short-circuits) | | | ➕ OIDC introspection `tls_client_auth` (RFC 8705 mTLS) — completes the five-of-five RFC 7662 §2.1 / RFC 8414 auth-method surface | ➕ OIDC introspection `self_signed_tls_client_auth` (RFC 8705 §2.2) — completes the six-of-six RFC 7662 §2.1 / RFC 8414 / RFC 8705 auth-method surface | ➕ `ChainedAuthResolver::with_short_circuit_on_transport_error` (opt-in fail-fast so OIDC-issuer-down doesn't mask as "unknown bearer token") | ➕ `ChainedAuthResolver::with_short_circuit_on_infrastructure_errors` — broader fail-fast covering `RateLimited` / `CircuitOpen` / `BudgetExhausted` | | | | | | ➕ OIDC mTLS cert/key rotation (`OidcAuthResolver::reload_mtls_identity` / `_combined`) — atomic-swap primitive for long-running deployments where cert-manager / Vault PKI / filesystem watchers refresh client certs without process restart |
| MCP client (stdio + Streamable HTTP) | ✅    |         |         | ➕ WS, gRPC | ➕ gRPC mTLS |  |         |         |         | | | ➕ Streamable HTTP SSE notifications + `Mcp-Session-Id` lifecycle | | | | | | | | | | | | | | | | | | | | | |
| `SingleAgent` orchestrator         | ✅      |         |         |         | ➕ budget |         |         |         |         | | | | | | | | | | | | | | | | | | | | | | | | |
| `Conductor` orchestrator           |         | ✅      |         |         |         | ➕ budget |         |         |         | ➕ verifier scores | | | | ➕ streaming `Verifier::evaluate_streaming` per-delta | | ➕ bounded `mpsc::channel(64)` worker fanout backpressure | | | | | | | | | | | | | | | | | |
| `Trinity` learned router           |         |         | ✅      |         |         | ➕ budget |         |         |         | ➕ verifier scores | | | ➕ streaming `Verifier::evaluate_streaming` | | | | | | | | | | | | | | | | | | | | |
| `SelfCaller` recursion             |         |         | ✅      |         |         | ➕ judge budget | ✅ native streaming | ➕ streaming guard | | | | | | | | | | | | | | | | | | | | | | | | | |
| `AbMcts` tree search               |         |         |         | ✅      |         |         |         | ✅ streaming + Python facade | ➕ router-driven branch expansion | | | | | | ➕ streaming `Verifier::evaluate_streaming` per-delta | ➕ bounded `mpsc::channel(64)` rollout-event backpressure | | | | | | | | | | | | | | | | | |
| Streaming guards (`ConfidenceGuard::evaluate_streaming`) | | | | | | | | ✅ rule-based early-abort | ➕ opt-in `LlmJudgeGuard` per-N-delta | | | | | | | | | | | | | | | | | | | | | | | | |
| Streaming verifier (`Verifier::evaluate_streaming`) | | | | | | | | | | | | | ✅ default-impl + Trinity per-delta + `RuleBasedVerifier` override | ➕ Conductor per-delta (worker fanout via mpsc) | ➕ AbMcts per-delta (rollout buffer + mpsc + `tokio::select!`) | ➕ bounded mpsc backpressure on AbMcts + Conductor channels | | | | | | | | | | | | | | | | | |
| OPA / Rego policy enforcement      |         | ✅      |         |         |         |         |         |         |         | | | | | | | | | | | | | | | | | | | | | | | | |
| PII / DLP redaction                | ✅      |         |         |         |         |         |         |         |         | | | | | | | | | | | | | | | | | | | | | | | | |
| OTel tracing (`tako.*`, `gen_ai.*`) | ✅     |         |         |         |         |         |         |         |         | | | | | | | | | | | | | | | | | | | | | | | | |
| Budgets (in-memory)                | ✅      |         |         | ➕ Redis | ➕ SingleAgent wiring | ➕ Conductor / Trinity / Judge | | | | | | | | | | | | | | | | | | | | | | | | | | | |
| Circuit breakers + rate limits     | ✅      |         |         |         |         |         |         |         |         | | | | | | | | | | | | | | | | | | | | | | | | |
| Sigstore tool-catalogue verify     |         |         |         | ✅ keyed | ➕ keyless | ➕ chain + Rekor SET | ➕ Rekor inclusion proof + cosign protobuf bundle | ➕ Rekor checkpoint | ➕ checkpoint freshness anchor | ➕ on-disk `JsonStateStore` | ➕ review-driven hardening (race-free anchor; `0o600` state file; `BasicConstraints` + critical-ext checks) | | ➕ `StateStore` trait + `RedisStateStore` (multi-replica) | | | | | | | | | | | | | | | | | | | | |
| Sync + async dual API              | ✅      |         |         |         |         |         |         |         |         | | | | | | | | | | | | | | | | | | | | | | | | |



## How to read a row

Take `LlmProvider trait + adapters` as an example:

- **Phase 1** (✅) — the trait and the first three adapters (Anthropic,
  OpenAI, http-generic) shipped together.
- **Phase 2.5** (➕) — added Azure, Bedrock, Vertex.
- **Phase 4** (➕) — added Mistral, Ollama (six-of-six provider sweep
  finished here for tool calls + streaming).
- **Phase 19 / 20** (➕) — added vision content (Anthropic + OpenAI in
  Phase 19; Vertex + Mistral + Ollama in Phase 20).
- **Phase 22 / 23 / 28** (➕) — added URL-source images (Anthropic +
  OpenAI + Mistral first via vendor-fetch; Vertex via `gs://`/`https://`
  `fileData`; Bedrock + Ollama via opt-in tako-side pre-fetch).
- **Phase 29 / 30 / 31 / 32** (➕) — locked down the URL pre-fetch
  surface with private-IP blocklist, DNS-rebind defence, and per-host /
  wildcard / CIDR allowlist forms.

The columns are dense because the feature evolved across many phases.
Sparse rows (e.g. PII / DLP redaction, sync + async dual API) shipped
once in Phase 1 and haven't needed extension.

## See also

- [`PLAN.md`](https://github.com/nyankobu010/tako-ai-core/blob/main/PLAN.md)
  — rolling project index with one row per phase pointing at its
  individual `PLAN_PHASE<N>.md`.
- [`CHANGELOG.md`](https://github.com/nyankobu010/tako-ai-core/blob/main/CHANGELOG.md)
  — release notes per `v0.X.0`.
- [Roadmap on the project home page](index.md#roadmap) — phase →
  version → theme summary.
