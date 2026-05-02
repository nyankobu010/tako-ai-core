# Sigstore tool-catalogue verification

`tako-governance` ships a Sigstore verifier so an MCP tool catalogue
can carry a Sigstore signature, and tako will refuse to load any tool
whose signature doesn't verify against an operator-pinned trust root.

The implementation lives in
[`crates/tako-governance/src/sigstore`](https://github.com/nyankobu010/tako-ai-core/blob/main/crates/tako-governance/src/sigstore.rs).

## Two modes

### Keyed verification

Operators pin a public key (or a fixed set of keys) and tako verifies
the signature directly. This is the simplest path; suitable when you
control the signing identity end-to-end.

```python
verifier = tako.sigstore.KeyedVerifier(
    public_keys=[ed25519_pubkey_pem],
)
client = tako.Client(
    providers=[...],
    sigstore_verifier=verifier,
)
```

### Keyless verification (Fulcio + Rekor)

Standard Sigstore — leaf certs are issued by Fulcio against an OIDC
identity (e.g. a GitHub Actions workload identity), the inclusion is
recorded in Rekor, and tako verifies:

1. The signature against the leaf cert's public key.
2. The leaf cert chain against an operator-pinned **`TrustRoot`** (so a
   compromised Fulcio CA can't unilaterally issue valid leaves).
3. The leaf cert SAN against an operator-supplied **identity policy**
   (e.g. "accept only `github-actions://nyankobu010/tako-ai-core`").
4. The Rekor SET (Signed Entry Timestamp).
5. The Rekor inclusion proof (Merkle audit-path) against a Rekor
   checkpoint.
6. The checkpoint's freshness against a TOFU water-mark — once a
   `tree_size` is recorded, future verifications accept only checkpoints
   with `tree_size ≥ that mark`. This blocks split-view attacks where a
   compromised log might roll back to an older state.

```python
verifier = tako.sigstore.KeylessVerifier(
    trust_root=tako.sigstore.TrustRoot.from_pem(rekor_root_pem, fulcio_root_pem),
    identity_policy=tako.sigstore.IdentityPolicy(
        san_pattern="github-actions://nyankobu010/tako-ai-core",
        oidc_issuer="https://token.actions.githubusercontent.com",
    ),
    state_store=tako.sigstore.JsonStateStore("/var/lib/tako/sigstore.json"),
)
```

## State stores

The Rekor checkpoint freshness anchor needs persistence. Three
`StateStore` impls ship:

| Backend | Use case |
|---------|----------|
| `InMemoryStateStore` | tests; single-process throwaway runs |
| `JsonStateStore` | single-process production. Crash-safe atomic writes, `0o600` mode on Unix, `tempfile::NamedTempFile` for collision-free swaps. |
| `RedisStateStore` | multi-replica production. A small Lua script enforces monotonic write so a slow replica cannot clobber a higher water-mark — the cross-process analogue of the in-process `fetch_max`. Behind the `tako-governance/redis` cargo feature. |

Python facade mirrors at `tako.sigstore.{InMemoryStateStore,
JsonStateStore, RedisStateStore}`.

## Cosign protobuf-bundle adapter

Cosign emits tool-catalogue signatures as a protobuf bundle bundling
cert + sig + Rekor entry into one file. tako's
`KeylessBundle::from_protobuf_bundle` parses that format directly so
operators can hand the raw `cosign sign-blob` output to tako without an
intermediate format conversion.

## What gets enforced

The verifier hard-fails when:

- The signature doesn't verify against the leaf cert.
- The leaf cert chain doesn't verify against the `TrustRoot`.
- The leaf cert SAN doesn't match the `IdentityPolicy`.
- `BasicConstraints: cA=TRUE` is missing or `pathLenConstraint` is
  violated on any intermediate.
- A non-whitelisted critical extension appears anywhere in the chain.
- The Rekor SET is invalid.
- The Rekor inclusion proof doesn't reconstruct the checkpoint root.
- The checkpoint's `tree_size` is below the freshness anchor's
  high-water-mark.

## See also

- [recipes/sigstore_keyless.md](../recipes/sigstore_keyless.md) — full
  walkthrough including a worked GitHub Actions signing config.
- [SECURITY_PHASE10.md](https://github.com/nyankobu010/tako-ai-core/blob/main/SECURITY_PHASE10.md)
  — the review-driven hardening notes for every Sigstore item shipped
  through Phase 11.
