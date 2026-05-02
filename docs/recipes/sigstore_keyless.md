# Sigstore keyless verification

Verify an MCP tool catalogue's Sigstore signature against a Fulcio
issuer + Rekor transparency log, with operator-pinned trust roots and
identity policy.

## Wiring

```python
import tako, tako.sigstore

verifier = tako.sigstore.KeylessVerifier(
    trust_root=tako.sigstore.TrustRoot.from_pem(
        rekor_root_pem=open("rekor-root.pem").read(),
        fulcio_root_pem=open("fulcio-root.pem").read(),
    ),
    identity_policy=tako.sigstore.IdentityPolicy(
        san_pattern="github-actions://nyankobu010/tako-ai-core",
        oidc_issuer="https://token.actions.githubusercontent.com",
    ),
    state_store=tako.sigstore.JsonStateStore("/var/lib/tako/sigstore.json"),
)

client = tako.Client(
    providers=[...],
    sigstore_verifier=verifier,
)
```

## What gets enforced

- The signature verifies against the leaf cert.
- The leaf cert chain verifies against the pinned `TrustRoot`.
- `BasicConstraints: cA=TRUE` + `pathLenConstraint` + critical-extension
  whitelist enforced on every intermediate.
- The leaf's SAN list is iterated — attacker-injected SANs cannot win
  the predicate (the iteration looks for *all* matches, and the policy
  must accept the one matching SAN).
- The Rekor SET (Signed Entry Timestamp) verifies.
- The Rekor inclusion proof reconstructs the checkpoint root.
- The checkpoint's `tree_size` is at or above the persisted
  freshness-anchor high-water-mark — once a value is recorded, future
  verifications accept only checkpoints with `tree_size ≥ that mark`.
  This blocks split-view attacks where a compromised log might serve
  one client a rolled-back state.

## Multi-replica deployments

Use `RedisStateStore` instead of `JsonStateStore` so all replicas share
one freshness anchor:

```python
verifier = tako.sigstore.KeylessVerifier(
    trust_root=...,
    identity_policy=...,
    state_store=tako.sigstore.RedisStateStore(
        redis_url="redis://shared-redis:6379/0",
        key="tako:sigstore:freshness",
    ),
)
```

The Redis backend uses a Lua script for monotonic-write semantics so a
slow replica cannot clobber a higher water-mark — the cross-process
analogue of the `JsonStateStore`'s `compare_exchange_weak` advance.

## Cosign protobuf-bundle adapter

If the catalogue is signed with `cosign sign-blob --bundle ...`, use:

```python
bundle = tako.sigstore.KeylessBundle.from_protobuf_bundle(
    open("catalogue.bundle", "rb").read(),
)
verifier.verify_bundle(bundle, payload=open("catalogue.json", "rb").read())
```

## See also

- [Concepts → Sigstore](../concepts/sigstore.md)
- [`SECURITY_PHASE10.md`](https://github.com/nyankobu010/tako-ai-core/blob/main/plans/SECURITY_PHASE10.md)
  — the review-driven hardening notes for every Sigstore item shipped
  through Phase 11.
