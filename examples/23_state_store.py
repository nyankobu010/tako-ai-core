"""Phase 10.A: on-disk JsonStateStore for Rekor freshness.

`tako.sigstore.JsonStateStore` persists `KeylessVerifier`'s in-memory
high-water mark across process restarts. The schema is a tiny JSON
object: `{"rekor_min_tree_size": <u64>}`. Saves use the standard
write-temp-then-rename pattern so an interrupted save never leaves a
corrupt anchor.

Typical operator pattern:

    store = tako.sigstore.JsonStateStore("/var/lib/tako/rekor.json")
    verifier = store.seed(tako.sigstore.KeylessVerifier(issuer, san))
    # ... verify bundles in your normal flow ...
    store.persist(verifier)

This example focuses on the file I/O round-trip; the seed/verify/persist
cycle against a real bundle is exercised by the Rust integration test
``crates/tako-governance/tests/sigstore.rs::state_store_seed_persist``.
"""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

import tako


def main() -> None:
    # Phase 11.A H2 — production deployments should set `umask 077`
    # for the parent directory so tako's `JsonStateStore` lands the
    # state file 0600 (per Phase 11.A's chmod) under a 0700 dir, not
    # under the default umask-022 0755 dir. The chmod on the file
    # itself is automatic; only the parent dir's permission is the
    # operator's responsibility.
    os.umask(0o077)
    with tempfile.TemporaryDirectory() as tmp:
        anchor = Path(tmp) / "rekor-anchor.json"
        store = tako.sigstore.JsonStateStore(str(anchor))

        # First boot: nothing on disk yet — load returns 0 (the
        # verifier's "uninitialised" sentinel).
        print(f"first boot load: {store.load()}")  # 0

        # Save a high-water mark and read it back.
        store.save(4711)
        print(f"after save(4711): {store.load()}")  # 4711
        print(f"on disk: {anchor.read_text()}")

        # Seed a fresh verifier. The persisted value is applied
        # in-place; `seed` returns the same verifier for chaining.
        verifier = store.seed(
            tako.sigstore.KeylessVerifier(
                issuer="https://accounts.example.com",
                san="ci@example.com",
            )
        )
        print(f"verifier rekor_max_tree_size: {verifier.rekor_max_tree_size()}")

        # `persist(verifier)` reads `rekor_max_tree_size()` and writes
        # it via `save`. After a successful verify cycle, this is the
        # one call to make in your shutdown path.
        store.persist(verifier)
        print(f"after persist: {store.load()}")


if __name__ == "__main__":
    main()
