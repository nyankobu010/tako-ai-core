"""Phase 11.A: hardened JsonStateStore round-trip.

Shows the file-confidentiality posture introduced in v0.12.0:

- Set `umask 0o077` on the process so the parent directory is created
  `0700` rather than the default `0755`.
- Call `JsonStateStore.save` (or `persist`) — tako automatically
  ``chmod 0o600`` the file after the atomic replace.
- The file's mode round-trips through `os.stat` to prove the
  posture, so a co-tenant on the same host cannot silently
  downgrade `rekor_min_tree_size`.

This example is a Phase-10 superset: it does the same seed/verify/
persist round-trip as ``examples/23_state_store.py`` but adds the
permission-mode assertions that make sense once the v0.12.0 chmod
landed.
"""

from __future__ import annotations

import os
import platform
import stat
import tempfile
from pathlib import Path

import tako


def main() -> None:
    # Phase 11.A H2 — clamp the umask before any file is created.
    os.umask(0o077)

    with tempfile.TemporaryDirectory() as tmp:
        anchor = Path(tmp) / "rekor-anchor.json"
        store = tako.sigstore.JsonStateStore(str(anchor))

        # First boot — no file on disk; load returns 0.
        print(f"first boot load: {store.load()}")

        # Save a high-water mark.
        store.save(4711)
        print(f"after save(4711): {store.load()}")

        # On Unix, assert the file mode is 0o600. On Windows the
        # chmod is a no-op and confidentiality is operator-managed
        # via NTFS ACLs on the parent directory.
        if platform.system() in {"Linux", "Darwin"}:
            mode = stat.S_IMODE(os.stat(anchor).st_mode)
            print(f"state file mode: {oct(mode)}")
            assert mode == 0o600, f"expected 0o600, got {oct(mode)}"

        # Round-trip seed → persist with a fresh verifier.
        verifier = store.seed(
            tako.sigstore.KeylessVerifier(
                issuer="https://accounts.example.com",
                san="ci@example.com",
            )
        )
        print(f"verifier rekor_max_tree_size: {verifier.rekor_max_tree_size()}")

        store.persist(verifier)
        print(f"after persist: {store.load()}")


if __name__ == "__main__":
    main()
