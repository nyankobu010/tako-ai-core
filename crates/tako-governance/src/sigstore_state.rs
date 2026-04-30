//! On-disk persistence helper for [`KeylessVerifier`]'s Rekor
//! checkpoint freshness anchor.
//!
//! Phase 9.B (v0.10.0) shipped the in-memory anchor on
//! [`KeylessVerifier`] — every successful `verify_bundle` asserts the
//! checkpoint's `tree_size` is monotonically non-decreasing within
//! the verifier's lifetime, but the high-water mark is lost on
//! process restart. Operators were expected to hand-roll seed/persist
//! around [`KeylessVerifier::with_rekor_min_tree_size`] /
//! [`KeylessVerifier::rekor_max_tree_size`].
//!
//! Phase 10.A ships that helper. [`JsonStateStore`] reads and writes
//! a tiny JSON file with the schema:
//!
//! ```json
//! { "rekor_min_tree_size": 4711 }
//! ```
//!
//! `save` is crash-safe via the standard `write-temp-then-rename`
//! pattern: the new value is written to `<path>.tmp` and `rename`d
//! over `<path>`, so an interrupted save cannot leave a corrupt
//! anchor file. `load` against a missing path returns `Ok(0)`,
//! matching the verifier's "uninitialised = no constraint"
//! sentinel.
//!
//! Two convenience methods, [`JsonStateStore::seed`] and
//! [`JsonStateStore::persist`], wrap the verifier handover so the
//! operator's startup / shutdown code stays tidy:
//!
//! ```no_run
//! # use tako_governance::sigstore::{IdentityPolicy, KeylessVerifier};
//! # use tako_governance::sigstore_state::JsonStateStore;
//! # let identity = IdentityPolicy::exact("https://accounts.example.com", "ci@example.com");
//! let store = JsonStateStore::new("/var/lib/tako/rekor.json");
//! let verifier = store.seed(KeylessVerifier::new(identity))?;
//! // ... verify bundles ...
//! store.persist(&verifier)?;
//! # Ok::<(), tako_core::TakoError>(())
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tako_core::TakoError;
use tempfile::NamedTempFile;

use crate::sigstore::KeylessVerifier;

/// On-disk JSON state for the [`KeylessVerifier::rekor_max_tree_size`]
/// freshness anchor.
#[derive(Debug, Clone)]
pub struct JsonStateStore {
    path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    rekor_min_tree_size: u64,
}

impl JsonStateStore {
    /// Build a store backed by `path`. The file is not touched until
    /// [`save`](Self::save) or [`persist`](Self::persist) is called.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The on-disk file backing this store.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the persisted `rekor_min_tree_size`. Returns `Ok(0)` when
    /// the file does not exist (first-boot semantics — the verifier
    /// treats `0` as "uninitialised, no constraint"). Other I/O or
    /// JSON-parse errors surface as [`TakoError::Invalid`].
    pub fn load(&self) -> Result<u64, TakoError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let parsed: StateFile = serde_json::from_slice(&bytes).map_err(|e| {
                    TakoError::Invalid(format!(
                        "sigstore_state: parse {}: {e}",
                        self.path.display()
                    ))
                })?;
                Ok(parsed.rekor_min_tree_size)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(TakoError::Invalid(format!(
                "sigstore_state: read {}: {e}",
                self.path.display()
            ))),
        }
    }

    /// Persist `n` as the new `rekor_min_tree_size` value. Writes to a
    /// per-call random tmp file in the same directory then atomically
    /// renames over `<path>` so a crash mid-write cannot leave a
    /// corrupt or partially-written anchor.
    ///
    /// Phase 11.A M1+M4 — `tempfile::NamedTempFile::new_in(parent)`
    /// generates a randomised suffix, so two concurrent `save()` calls
    /// on a shared `Arc<JsonStateStore>` cannot collide on the same
    /// tmp path. Its `Drop` impl deletes the tmp on the failure path
    /// (when `persist` is not reached), so a `rename` error never
    /// leaves an orphan `*.tmp` file behind.
    pub fn save(&self, n: u64) -> Result<(), TakoError> {
        let body = serde_json::to_vec(&StateFile {
            rekor_min_tree_size: n,
        })
        .map_err(|e| TakoError::Invalid(format!("sigstore_state: serialise: {e}")))?;

        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| {
            TakoError::Invalid(format!(
                "sigstore_state: create parent {}: {e}",
                parent.display()
            ))
        })?;

        let mut tmp = NamedTempFile::new_in(parent).map_err(|e| {
            TakoError::Invalid(format!(
                "sigstore_state: open tmp in {}: {e}",
                parent.display()
            ))
        })?;
        tmp.write_all(&body).map_err(|e| {
            TakoError::Invalid(format!(
                "sigstore_state: write tmp {}: {e}",
                tmp.path().display()
            ))
        })?;
        tmp.as_file_mut().sync_all().map_err(|e| {
            TakoError::Invalid(format!(
                "sigstore_state: fsync tmp {}: {e}",
                tmp.path().display()
            ))
        })?;
        tmp.persist(&self.path).map_err(|e| {
            // `persist` returns the original `NamedTempFile` on Err so
            // the `Drop` impl can clean up; we only need the message.
            TakoError::Invalid(format!(
                "sigstore_state: persist tmp -> {}: {}",
                self.path.display(),
                e.error,
            ))
        })?;
        Ok(())
    }

    /// Load the persisted high-water mark and seed `verifier` with it.
    /// Returns the consumed-and-rebuilt verifier so the call composes
    /// into a builder chain.
    pub fn seed(&self, verifier: KeylessVerifier) -> Result<KeylessVerifier, TakoError> {
        let n = self.load()?;
        Ok(verifier.with_rekor_min_tree_size(n))
    }

    /// Read `verifier.rekor_max_tree_size()` and write it via
    /// [`save`](Self::save). No-op semantics when the verifier's
    /// high-water mark is `0` (still writes `0`, so the next boot
    /// `load`s an explicit `0` rather than a missing file).
    pub fn persist(&self, verifier: &KeylessVerifier) -> Result<(), TakoError> {
        self.save(verifier.rekor_max_tree_size())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStateStore::new(dir.path().join("anchor.json"));
        store.save(7).unwrap();
        assert_eq!(store.load().unwrap(), 7);
    }

    #[test]
    fn first_boot_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStateStore::new(dir.path().join("missing.json"));
        assert_eq!(store.load().unwrap(), 0);
    }

    #[test]
    fn save_is_atomic_no_tmp_residue() {
        // Phase 11.A M1+M4 — `NamedTempFile` generates a randomised
        // suffix per call, so a residue check globs the parent dir
        // for any sibling that isn't the persisted state file itself.
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStateStore::new(dir.path().join("anchor.json"));
        store.save(42).unwrap();
        assert!(store.path().exists());
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "parent dir should contain only the persisted file, found: {entries:?}"
        );
        assert_eq!(entries[0], store.path());
    }

    #[test]
    fn save_creates_missing_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("anchor.json");
        let store = JsonStateStore::new(&nested);
        store.save(11).unwrap();
        assert_eq!(store.load().unwrap(), 11);
    }

    #[test]
    fn parse_error_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anchor.json");
        std::fs::write(&path, b"not-json").unwrap();
        let store = JsonStateStore::new(&path);
        let err = store.load().unwrap_err();
        match err {
            TakoError::Invalid(msg) => assert!(msg.contains("parse")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}
