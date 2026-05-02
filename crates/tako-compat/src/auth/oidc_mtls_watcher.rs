//! Phase 35 — filesystem-watcher integration for OIDC mTLS
//! cert/key auto-reload.
//!
//! Wraps the [`notify`] crate behind the `mtls-fs-watch` cargo
//! feature. Operators using cert-manager / Vault PKI / SPIRE /
//! kubernetes-secret-mount rotation call
//! [`OidcAuthResolver::watch_mtls_files`] once at startup; the
//! returned [`MtlsFsWatcher`] handle holds a background tokio
//! task that re-reads the cert+key files whenever they change
//! on disk and calls [`OidcAuthResolver::reload_mtls_identity`].
//!
//! Behaviour summary:
//!
//! - **Watches the parent directory** of each cert/key path —
//!   not the files themselves. Atomic-rename rotation (the
//!   pattern cert-manager and kubernetes-secret-mount use)
//!   replaces the inner inode, so a watch directly on the file
//!   path goes stale. Watching the parent dir + filtering by
//!   filename matches the inotify(7) recommendation. When the
//!   cert and key live in the same directory the watcher
//!   registers it once.
//! - **Coalesces bursts** with a 500 ms debounce. Cert-manager
//!   writes both cert and key in quick succession; a single
//!   reload per debounce window is the right cadence.
//! - **Reload errors do not kill the watcher.** A
//!   [`tracing::warn!`] line records the failure; the next
//!   change event triggers another attempt. The Phase 33
//!   `reload_mtls_identity` semantics already preserve the
//!   previously installed client on PEM-parse failure, so a
//!   transient mid-rotation invalid-PEM read does not break
//!   the running server.
//! - **No bootstrap reload.** The resolver was already
//!   constructed with `with_introspection_mtls(initial_cert,
//!   initial_key)`. The watcher's job is rotation, not
//!   bootstrap.
//! - **Drop is clean.** The `Drop` impl signals the background
//!   task and aborts its `JoinHandle`. The `notify::Watcher`
//!   itself stops emitting on drop.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tako_core::TakoError;
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
    time::Instant,
};

use crate::auth::oidc::OidcAuthResolver;

/// How long to wait after the most recent change event before
/// firing a reload. Cert-manager writes cert and key in quick
/// succession; debouncing collapses both into one reload.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Bound on the channel between the synchronous `notify`
/// callback and the async consumer task. 128 covers the burst
/// from a directory rewrite (~10s of events) with headroom.
/// Overflow is handled by dropping the oldest event — losing
/// individual events is harmless because we always re-read both
/// files at the next debounce fire.
const EVENT_CHANNEL_BOUND: usize = 128;

/// Handle for an active filesystem watcher. The watcher runs
/// until this handle is dropped (or [`shutdown`](Self::shutdown)
/// is called explicitly). Hold it for the lifetime of the
/// resolver — typically a module-scope variable in production.
pub struct MtlsFsWatcher {
    // Dropping the `RecommendedWatcher` un-registers the
    // platform-specific fs notification (inotify / kqueue /
    // ReadDirectoryChangesW / FSEvents).
    _watcher: RecommendedWatcher,
    shutdown: Arc<Notify>,
    join: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for MtlsFsWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsFsWatcher")
            .field(
                "running",
                &self.join.as_ref().is_some_and(|h| !h.is_finished()),
            )
            .finish()
    }
}

impl MtlsFsWatcher {
    /// Stop the watcher and await the background task's
    /// teardown. Idempotent — calling twice is a no-op the
    /// second time.
    pub async fn shutdown(mut self) {
        self.shutdown.notify_one();
        if let Some(h) = self.join.take() {
            h.abort();
            // Best-effort wait; the abort already cancelled the
            // task. We discard the JoinError that aborts produce.
            let _ = h.await;
        }
    }
}

impl Drop for MtlsFsWatcher {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        if let Some(h) = self.join.take() {
            h.abort();
        }
    }
}

impl OidcAuthResolver {
    /// Phase 35 — spawn a background task that watches
    /// `cert_path` and `key_path` for filesystem changes and
    /// auto-calls [`Self::reload_mtls_identity`] whenever
    /// either file is rewritten.
    ///
    /// The returned [`MtlsFsWatcher`] handle owns the
    /// `notify::Watcher` and the background task; dropping it
    /// (or calling [`MtlsFsWatcher::shutdown`]) stops the
    /// watcher. Operators typically bind it to a module-scope
    /// variable for the lifetime of the resolver.
    ///
    /// Errors:
    ///
    /// - No prior `with_introspection_mtls` /
    ///   `with_introspection_self_signed_mtls` call —
    ///   `TakoError::Invalid` with operator guidance.
    /// - Either parent directory is missing — `TakoError::Invalid`.
    /// - `notify::Watcher` setup fails (kernel limit,
    ///   permission, unsupported filesystem) — `TakoError::Invalid`
    ///   wrapping the underlying error.
    pub fn watch_mtls_files(
        self: Arc<Self>,
        cert_path: PathBuf,
        key_path: PathBuf,
    ) -> Result<MtlsFsWatcher, TakoError> {
        if !self.introspection_mtls_configured() {
            return Err(TakoError::Invalid(
                "oidc: watch_mtls_files called but no mTLS identity configured \
                 (call with_introspection_mtls or with_introspection_self_signed_mtls first)"
                    .into(),
            ));
        }

        let cert_dir = parent_dir(&cert_path)?;
        let key_dir = parent_dir(&key_path)?;

        // Channel from the sync `notify` callback to the async
        // consumer task. Bounded — see EVENT_CHANNEL_BOUND.
        let (tx, mut rx) = mpsc::channel::<Event>(EVENT_CHANNEL_BOUND);

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            match res {
                Ok(ev) => {
                    // try_send: drop on overflow rather than
                    // block the kernel notification thread.
                    // Lost events are harmless; the next event
                    // re-reads both files anyway.
                    let _ = tx.try_send(ev);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "oidc.mtls_fs_watcher: notify error");
                }
            }
        })
        .map_err(|e| {
            TakoError::Invalid(format!(
                "oidc: failed to construct mTLS filesystem watcher: {e}"
            ))
        })?;

        watcher
            .watch(&cert_dir, RecursiveMode::NonRecursive)
            .map_err(|e| {
                TakoError::Invalid(format!(
                    "oidc: failed to watch cert directory {}: {e}",
                    cert_dir.display()
                ))
            })?;
        // Watch key dir only if it differs — duplicate
        // registration is a no-op on most backends but a hard
        // error on others (kqueue).
        if key_dir != cert_dir {
            watcher
                .watch(&key_dir, RecursiveMode::NonRecursive)
                .map_err(|e| {
                    TakoError::Invalid(format!(
                        "oidc: failed to watch key directory {}: {e}",
                        key_dir.display()
                    ))
                })?;
        }

        let shutdown = Arc::new(Notify::new());
        let task_shutdown = shutdown.clone();
        let resolver = self.clone();
        let cert_path_owned = cert_path.clone();
        let key_path_owned = key_path.clone();

        let join = tokio::spawn(async move {
            run_watch_loop(
                resolver,
                cert_path_owned,
                key_path_owned,
                &mut rx,
                task_shutdown,
            )
            .await;
        });

        Ok(MtlsFsWatcher {
            _watcher: watcher,
            shutdown,
            join: Some(join),
        })
    }
}

/// Background task: receive events, debounce, reload.
async fn run_watch_loop(
    resolver: Arc<OidcAuthResolver>,
    cert_path: PathBuf,
    key_path: PathBuf,
    rx: &mut mpsc::Receiver<Event>,
    shutdown: Arc<Notify>,
) {
    let cert_name = leaf_filename(&cert_path);
    let key_name = leaf_filename(&key_path);
    let mut pending_deadline: Option<Instant> = None;

    loop {
        // Compute timeout: either the debounce deadline (if
        // an event is pending) or "wait forever" (if not).
        let recv = async {
            match pending_deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some(ev)) => RecvOutcome::Event(ev),
                    Ok(None) => RecvOutcome::Closed,
                    Err(_) => RecvOutcome::DebounceExpired,
                },
                None => match rx.recv().await {
                    Some(ev) => RecvOutcome::Event(ev),
                    None => RecvOutcome::Closed,
                },
            }
        };

        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                tracing::debug!("oidc.mtls_fs_watcher: shutdown received");
                return;
            }
            outcome = recv => match outcome {
                RecvOutcome::Event(ev) => {
                    if event_touches(&ev, &cert_name, &key_name) {
                        pending_deadline = Some(Instant::now() + DEBOUNCE);
                    }
                }
                RecvOutcome::DebounceExpired => {
                    pending_deadline = None;
                    do_reload(&resolver, &cert_path, &key_path);
                }
                RecvOutcome::Closed => {
                    tracing::debug!("oidc.mtls_fs_watcher: event channel closed");
                    return;
                }
            }
        }
    }
}

enum RecvOutcome {
    Event(Event),
    DebounceExpired,
    Closed,
}

/// Read both files and call `reload_mtls_identity`. Errors are
/// logged but do not propagate — the previously installed
/// Client stays in place per Phase 33 semantics.
fn do_reload(resolver: &Arc<OidcAuthResolver>, cert_path: &Path, key_path: &Path) {
    let cert = match std::fs::read(cert_path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                cert_path = %cert_path.display(),
                error = %e,
                "oidc.mtls_fs_watcher: failed to read cert file; skipping reload"
            );
            return;
        }
    };
    let key = match std::fs::read(key_path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                key_path = %key_path.display(),
                error = %e,
                "oidc.mtls_fs_watcher: failed to read key file; skipping reload"
            );
            return;
        }
    };
    match resolver.reload_mtls_identity(&cert, &key) {
        Ok(()) => {
            tracing::info!(
                cert_path = %cert_path.display(),
                key_path = %key_path.display(),
                "oidc.mtls_fs_watcher: reloaded mTLS identity from disk"
            );
        }
        Err(e) => {
            tracing::warn!(
                cert_path = %cert_path.display(),
                key_path = %key_path.display(),
                error = %e,
                "oidc.mtls_fs_watcher: reload_mtls_identity failed; previous client preserved"
            );
        }
    }
}

/// Filter: does this event touch either watched filename?
///
/// Accepts the whole Create / Modify / Remove family (rather
/// than precise sub-kinds) because notify's per-platform
/// backends emit different shapes for the same logical
/// rotation:
///
/// - **Linux inotify** — in-place rewrite is `Modify(Data(Content))`;
///   atomic rename is `Modify(Name(From))` + `Modify(Name(To))`.
/// - **macOS FSEvents** — atomic rename is `Modify(Name(Any))`
///   on both source and destination paths; in-place rewrite
///   coalesces into `Modify(Any)` or `Modify(Data(Content))`.
/// - **Windows ReadDirectoryChangesW** — atomic rename emits
///   `Create(File)` on the destination and `Remove(File)` on
///   the source.
///
/// We filter only by filename and by the broad Create / Modify
/// / Remove kind family, then trust the 500 ms debounce to
/// coalesce bursts. `Access(_)` events (read-only opens) and
/// `Other` are filtered out so an unrelated `cat` of the cert
/// file doesn't reset the debounce window for nothing.
fn event_touches(ev: &Event, cert_name: &str, key_name: &str) -> bool {
    let kind_relevant = matches!(
        &ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    );
    if !kind_relevant {
        return false;
    }
    ev.paths.iter().any(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name == cert_name || name == key_name)
    })
}

fn parent_dir(path: &Path) -> Result<PathBuf, TakoError> {
    let parent = path.parent().ok_or_else(|| {
        TakoError::Invalid(format!(
            "oidc: cannot watch path with no parent: {}",
            path.display()
        ))
    })?;
    if !parent.is_dir() {
        return Err(TakoError::Invalid(format!(
            "oidc: parent directory does not exist: {}",
            parent.display()
        )));
    }
    Ok(parent.to_path_buf())
}

fn leaf_filename(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    /// Self-signed X.509 cert generated for the Phase 24 tests.
    /// CN = `tako-test-client`, RSA-2048, validity ~100 years.
    const TEST_MTLS_CERT_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\n\
MIIDGTCCAgGgAwIBAgIUDgBxyYdSvB715hZ2wo2vg58ajPEwDQYJKoZIhvcNAQEL\n\
BQAwGzEZMBcGA1UEAwwQdGFrby10ZXN0LWNsaWVudDAgFw0yNjA1MDEwODQyNDJa\n\
GA8yMTI2MDQwNzA4NDI0MlowGzEZMBcGA1UEAwwQdGFrby10ZXN0LWNsaWVudDCC\n\
ASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAKn2gHS5FrOc6Kjx1aZDzmpB\n\
3CLeVWMYfXtVJO+p6mtJkIYUhqPLt1BesbdABmIBByjghtlenEP9xbOYbEe5qPxQ\n\
ihy9VmgITrq3DXUhdZhCxGHp99dzLPaE1XBaUHH3eYqlbbxd8dc1qRiULA6/f7mR\n\
92q6sZzUp5znDRwvwRGgf0x3JowfzeIetoKtNJ/RH1LmyCeqGd1djtyVe/2atsbL\n\
6DfDoEdT4en0WcIkZGtw9LYKvTImCidqTd8N+dpNSMPJTn4KctVHXmOdBpDK5U/u\n\
XF+SsGFg+4lFO/JTGTCowBGv7KeIoBf5vrJe9w/L01rCExnZhYQVTs8wjNZ1VBsC\n\
AwEAAaNTMFEwHQYDVR0OBBYEFEqlQohkpj9ddcSoQ57Onk0c5iwGMB8GA1UdIwQY\n\
MBaAFEqlQohkpj9ddcSoQ57Onk0c5iwGMA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZI\n\
hvcNAQELBQADggEBACtlr1SIz6YsRijnj9oMhGei1CVRXRnFHD8z2poa7A1Zh3vC\n\
nFdBOACHpmJ++A8Z1xOFyM064U/lYNybFw0/kyhk+9x5LlV3XCnT2r3CjVeacyfF\n\
kWy8kmaZ2j6JRTL/O0j8+ZlSZkf4utt/3+uGFUQ/qmmnXsYbhsyvHpnUmhZAnQxc\n\
Y+zVlpb9xALf3F2RuHZmhngdbIBaRFuExhcnktIdHbUUCq+Lc45or0gCk1yqf2GX\n\
+PIVp3MWA9hwQP3Obx88GzGaLZ/MpfzE41vVjtlnyBirt0lFqAyM8JT+vFjcrg0n\n\
ZVBd2WsafuufFwi8IZInM7P/gTi57eNbhpQMYzc=\n\
-----END CERTIFICATE-----\n";

    /// PKCS#8 RSA private key matching [`TEST_MTLS_CERT_PEM`].
    const TEST_MTLS_KEY_PEM: &[u8] = b"-----BEGIN PRIVATE KEY-----\n\
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCp9oB0uRaznOio\n\
8dWmQ85qQdwi3lVjGH17VSTvqeprSZCGFIajy7dQXrG3QAZiAQco4IbZXpxD/cWz\n\
mGxHuaj8UIocvVZoCE66tw11IXWYQsRh6ffXcyz2hNVwWlBx93mKpW28XfHXNakY\n\
lCwOv3+5kfdqurGc1Kec5w0cL8ERoH9MdyaMH83iHraCrTSf0R9S5sgnqhndXY7c\n\
lXv9mrbGy+g3w6BHU+Hp9FnCJGRrcPS2Cr0yJgonak3fDfnaTUjDyU5+CnLVR15j\n\
nQaQyuVP7lxfkrBhYPuJRTvyUxkwqMARr+yniKAX+b6yXvcPy9NawhMZ2YWEFU7P\n\
MIzWdVQbAgMBAAECggEAUnySalO7207/MaMw5AELiFFPY9LQ0Qe9OqKfivtFjG1H\n\
CXOjxpHjdUuH555Ymq7SCToy6AL9Rxg+H4QNpR/Lji0OYpVXfqTthLu7ecnT1yIs\n\
SjLxeGxq+XeNWPpUCYOoRqwz3lQfv6lI2GdtHHk/JVJcqD1UXv9sG3+dQr1Ab+tQ\n\
tVmVRNHA7E297v5kwYjKxEvobBjtRqDS3mVh21Fcfd+YNvAzbQ5MJpc1fqJ6TzLD\n\
4vs3yNZ2Utww4ItMFi1jf4AGxJ+s9887rJffV96g8fmaAAVJPHX6aHj+J2yibLiY\n\
TBpImZgd5x/sis9nNQdfA4749gb/vn/d+wt5Nq8o4QKBgQDbHtW/eDIJzQrj4djB\n\
pJXvGiQzp7dwgd5zxjpRmMpWMOymyJfu4LOW0hGH+YOmKV4DTdJ0OsNlpnccsQFT\n\
d0Xnpbmz0KXDybaUwqEsExpkNiPruC3Nq5ID03l89q1usyoLZfYPSWESfazmjG6h\n\
VlS2kKLwrTK3pLdKWbEBLIptewKBgQDGkaICZdgLbu5zbTszUi0zjZuVH59lIu3+\n\
CrxgGdjPTyCrot0qzxiYWnc6auvm8VVKoO8YqWyGaknwUmI8AwU9tETgTh4cv6gu\n\
YzOr6EhBYfNkUoTAkdyDu7Vbje7zSJY8YsjJCrdazj5gIOq4hLazXE9JFQnBoWln\n\
BQjXehbh4QKBgQDDHiQMCYXVQGZwIc4YMOzqKwcNkE1CvAJQabXIrxuNwKcapQjV\n\
x/VjWdAOmtrl/XQf0Q6UPTd9rsvmGqApqM3wxpwkSKkzPM1+jgli6+fWUHeQEUOI\n\
Hz04dvl5k1dAef34hGSlnBv6kTqDWY2x0ORCZW0Sj8fXy68DX/bEKtthPQKBgExe\n\
NDXB334+Mrz31J3fS/0YyC5pFA98iJV8oYhASI8qeoEoSPEu5uGpYVN5TbLrPAdQ\n\
r8QHXPKxLDCeLqOv8bMSgq7VvGUIHPGCO5ww4KEsv8PkrKO3NV0AszY79xtf3k/p\n\
Ghmf4nas/XZREpTWjcGbje6ohbEPmA8D86uTi/thAoGAZVuIdoETvKNVpT0O0qBX\n\
yxjBYrLoXdkns6ZR5I+jD42jvtv9UkASFydzHI6k5ZCJ38HN7hRoLHCSB46cEcOX\n\
GyKEFEUINrmViWeq1ysFaNzOu0EjypVCwvN6/Jx8kmfNFuHGdpqjoaNRecAYyGOr\n\
h7ACptP3tF94pcBzOgJ3bhM=\n\
-----END PRIVATE KEY-----\n";

    fn build_resolver_with_mtls() -> OidcAuthResolver {
        OidcAuthResolver::new_for_internal_testing(
            "https://issuer.example",
            "test-aud",
            Some("https://issuer.example/introspect"),
        )
        .with_introspection("client-id", Some("client-secret".into()))
        .expect("introspection builds against discovered uri")
        .with_introspection_self_signed_mtls(TEST_MTLS_CERT_PEM, TEST_MTLS_KEY_PEM)
        .expect("self-signed mTLS builds")
    }

    /// Polls `pred` every 50 ms up to `total`. Returns `true` if
    /// `pred` ever observed true. Used in lieu of an explicit
    /// reload-completed signal.
    async fn wait_for(total: Duration, mut pred: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + total;
        while std::time::Instant::now() < deadline {
            if pred() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        pred()
    }

    fn write_atomic(path: &Path, bytes: &[u8]) {
        let mut tmp = path.to_path_buf();
        tmp.set_extension("tmp");
        std::fs::write(&tmp, bytes).expect("write tmp");
        std::fs::rename(&tmp, path).expect("atomic rename");
    }

    fn current_client_ptr(resolver: &OidcAuthResolver) -> *const reqwest::Client {
        let arc = resolver
            .introspection_mtls_client_arc()
            .expect("mTLS configured");
        Arc::as_ptr(&arc.current())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cert_change_triggers_reload() {
        let dir = TempDir::new().expect("tempdir");
        let cert_path = dir.path().join("client.crt");
        let key_path = dir.path().join("client.key");
        write_atomic(&cert_path, TEST_MTLS_CERT_PEM);
        write_atomic(&key_path, TEST_MTLS_KEY_PEM);

        let resolver = Arc::new(build_resolver_with_mtls());
        let initial = current_client_ptr(&resolver);

        let _watcher = resolver
            .clone()
            .watch_mtls_files(cert_path.clone(), key_path.clone())
            .expect("watcher constructs");

        // Give notify a moment to register the watch.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Atomic rotation — same bytes, but the inode flips.
        write_atomic(&cert_path, TEST_MTLS_CERT_PEM);

        let swapped = wait_for(Duration::from_secs(3), || {
            current_client_ptr(&resolver) != initial
        })
        .await;
        assert!(
            swapped,
            "expected MtlsClient inner Arc to swap after cert rewrite"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn key_change_triggers_reload() {
        let dir = TempDir::new().expect("tempdir");
        let cert_path = dir.path().join("client.crt");
        let key_path = dir.path().join("client.key");
        write_atomic(&cert_path, TEST_MTLS_CERT_PEM);
        write_atomic(&key_path, TEST_MTLS_KEY_PEM);

        let resolver = Arc::new(build_resolver_with_mtls());
        let initial = current_client_ptr(&resolver);

        let _watcher = resolver
            .clone()
            .watch_mtls_files(cert_path.clone(), key_path.clone())
            .expect("watcher constructs");

        tokio::time::sleep(Duration::from_millis(200)).await;

        write_atomic(&key_path, TEST_MTLS_KEY_PEM);

        let swapped = wait_for(Duration::from_secs(3), || {
            current_client_ptr(&resolver) != initial
        })
        .await;
        assert!(
            swapped,
            "expected MtlsClient inner Arc to swap after key rewrite"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn parse_failure_preserves_client() {
        let dir = TempDir::new().expect("tempdir");
        let cert_path = dir.path().join("client.crt");
        let key_path = dir.path().join("client.key");
        write_atomic(&cert_path, TEST_MTLS_CERT_PEM);
        write_atomic(&key_path, TEST_MTLS_KEY_PEM);

        let resolver = Arc::new(build_resolver_with_mtls());
        let initial = current_client_ptr(&resolver);

        let _watcher = resolver
            .clone()
            .watch_mtls_files(cert_path.clone(), key_path.clone())
            .expect("watcher constructs");

        tokio::time::sleep(Duration::from_millis(200)).await;

        write_atomic(
            &cert_path,
            b"-----BEGIN CERTIFICATE-----\nGARBAGE\n-----END CERTIFICATE-----\n",
        );

        // Past the debounce window plus headroom — no swap should
        // have happened because reload_mtls_identity rejects the
        // garbage PEM.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            current_client_ptr(&resolver),
            initial,
            "client must not swap when PEM parse fails"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_stops_watcher() {
        let dir = TempDir::new().expect("tempdir");
        let cert_path = dir.path().join("client.crt");
        let key_path = dir.path().join("client.key");
        write_atomic(&cert_path, TEST_MTLS_CERT_PEM);
        write_atomic(&key_path, TEST_MTLS_KEY_PEM);

        let resolver = Arc::new(build_resolver_with_mtls());
        let initial = current_client_ptr(&resolver);

        {
            let _w: MtlsFsWatcher = resolver
                .clone()
                .watch_mtls_files(cert_path.clone(), key_path.clone())
                .expect("watcher constructs");
            // Drop scope ends here.
        }

        write_atomic(&cert_path, TEST_MTLS_CERT_PEM);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            current_client_ptr(&resolver),
            initial,
            "dropped watcher must not have reloaded the client"
        );
    }

    #[tokio::test]
    async fn errors_when_no_mtls_configured() {
        let dir = TempDir::new().expect("tempdir");
        let cert_path = dir.path().join("client.crt");
        let key_path = dir.path().join("client.key");
        write_atomic(&cert_path, TEST_MTLS_CERT_PEM);
        write_atomic(&key_path, TEST_MTLS_KEY_PEM);

        let resolver = Arc::new(OidcAuthResolver::new_for_internal_testing(
            "https://issuer.example",
            "test-aud",
            Some("https://issuer.example/introspect"),
        ));
        let err = resolver
            .watch_mtls_files(cert_path, key_path)
            .expect_err("should fail without mTLS config");
        let msg = format!("{err}");
        assert!(
            msg.contains("no mTLS identity configured"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn errors_when_parent_dir_missing() {
        let dir = TempDir::new().expect("tempdir");
        let cert_path = dir.path().join("nope/client.crt");
        let key_path = dir.path().join("nope/client.key");
        let resolver = Arc::new(build_resolver_with_mtls());
        let err = resolver
            .watch_mtls_files(cert_path, key_path)
            .expect_err("should fail on missing parent dir");
        let msg = format!("{err}");
        assert!(
            msg.contains("parent directory does not exist"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn event_touches_matches_atomic_rename() {
        use notify::event::CreateKind;
        let ev = Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![std::path::PathBuf::from("/tmp/foo/client.crt")],
            attrs: Default::default(),
        };
        assert!(event_touches(&ev, "client.crt", "client.key"));
        assert!(!event_touches(&ev, "other.crt", "other.key"));
    }

    #[test]
    fn event_touches_matches_macos_fsevents_rename() {
        use notify::event::{ModifyKind, RenameMode};
        let ev = Event {
            kind: EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            paths: vec![std::path::PathBuf::from("/tmp/foo/client.key")],
            attrs: Default::default(),
        };
        assert!(event_touches(&ev, "client.crt", "client.key"));
    }

    #[test]
    fn event_touches_ignores_unrelated_kinds() {
        let ev = Event {
            kind: EventKind::Access(notify::event::AccessKind::Read),
            paths: vec![std::path::PathBuf::from("/tmp/foo/client.crt")],
            attrs: Default::default(),
        };
        assert!(!event_touches(&ev, "client.crt", "client.key"));
    }
}
