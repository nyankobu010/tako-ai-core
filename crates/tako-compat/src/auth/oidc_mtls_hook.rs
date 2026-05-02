//! Phase 39 — refresh-hook primitive for auto-retry on TLS
//! handshake failure during OIDC introspection POSTs.
//!
//! The hook is a one-shot RPC channel between the
//! [`OidcAuthResolver::introspect`](crate::auth::OidcAuthResolver)
//! retry layer and a Phase 35 filesystem watcher / Phase 37
//! trait-based provider's background task. When the
//! introspection POST hits a [`TakoError::Transport`], the
//! retry layer calls
//! [`MtlsRefreshHook::force_refresh`]; the watcher's task
//! receives the trigger, performs an out-of-band reload, and
//! signals back via the per-call `oneshot`. The retry layer
//! then re-sends the POST exactly once.
//!
//! Cycle-detection is structural: the retry path calls the
//! send helper only one more time, never recursing into the
//! retry layer.

use std::sync::Arc;
use std::time::Duration;

use tako_core::TakoError;
use tokio::sync::{mpsc, oneshot};

/// Phase 39 — cap on the per-call refresh wait. A
/// well-behaved watcher / provider responds in <100ms; 2s
/// covers slow HSM RPCs without making the introspection POST
/// retry feel hung.
pub(crate) const REFRESH_TIMEOUT: Duration = Duration::from_secs(2);

/// Phase 39 — one-shot reply carried over the trigger
/// channel. The watcher / provider task signals success or
/// the underlying reload error back to the caller.
pub(crate) type RefreshReply = oneshot::Sender<Result<(), TakoError>>;

/// Phase 39 — receiver side of the refresh channel; consumed
/// by the watcher / provider's `select!` loop.
#[allow(dead_code)] // wired by Phase 39 watcher / provider integrations
pub(crate) struct MtlsRefreshTrigger {
    pub(crate) trigger_rx: mpsc::Receiver<RefreshReply>,
}

/// Phase 39 — handle that triggers an out-of-band mTLS
/// identity refresh from a Phase 35 filesystem watcher or a
/// Phase 37 trait-based provider.
///
/// Operators rarely construct this directly. Both
/// [`MtlsFsWatcher::refresh_hook`](super::oidc_mtls_watcher::MtlsFsWatcher::refresh_hook)
/// and
/// [`MtlsProviderWatcher::refresh_hook`](super::oidc_mtls_provider::MtlsProviderWatcher::refresh_hook)
/// return a fully-wired hook that drives the corresponding
/// watcher's background task. Pass the hook to
/// [`OidcAuthResolver::with_mtls_refresh_hook`](super::oidc::OidcAuthResolver::with_mtls_refresh_hook)
/// to enable auto-retry on TLS-handshake failure.
#[derive(Clone)]
pub struct MtlsRefreshHook {
    inner: Arc<MtlsRefreshHookInner>,
}

impl std::fmt::Debug for MtlsRefreshHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsRefreshHook").finish_non_exhaustive()
    }
}

struct MtlsRefreshHookInner {
    trigger_tx: mpsc::Sender<RefreshReply>,
}

impl MtlsRefreshHook {
    /// Trigger an out-of-band reload from the wired refresh
    /// source and await the source's response.
    ///
    /// Returns:
    ///
    /// - `Ok(())` when the underlying source successfully
    ///   reloaded the mTLS identity.
    /// - `Err(TakoError::Invalid("...refresh source dropped..."))`
    ///   when the watcher / provider task has stopped (drop /
    ///   shutdown).
    /// - `Err(TakoError::Invalid("...refresh timed out..."))`
    ///   when the source doesn't respond within
    ///   [`REFRESH_TIMEOUT`] (2s).
    /// - The source's own error otherwise (PEM parse failure,
    ///   provider fetch error, etc.) — Phase 33 semantics
    ///   still preserve the previously installed Client.
    pub async fn force_refresh(&self) -> Result<(), TakoError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.inner.trigger_tx.send(resp_tx).await.map_err(|_| {
            TakoError::Invalid(
                "oidc.mtls_refresh_hook: refresh source dropped (watcher / provider task is gone)"
                    .into(),
            )
        })?;
        match tokio::time::timeout(REFRESH_TIMEOUT, resp_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(TakoError::Invalid(
                "oidc.mtls_refresh_hook: refresh source dropped reply channel".into(),
            )),
            Err(_) => Err(TakoError::Invalid(format!(
                "oidc.mtls_refresh_hook: refresh timed out after {REFRESH_TIMEOUT:?}"
            ))),
        }
    }
}

/// Phase 39 — construct a paired refresh-hook + trigger.
/// `pub(crate)` because operators don't call this directly;
/// `MtlsFsWatcher` / `MtlsProviderWatcher` invoke it during
/// their own constructors and expose the hook via
/// `refresh_hook()`.
#[allow(dead_code)] // wired by Phase 39 watcher / provider integrations
pub(crate) fn refresh_channel() -> (MtlsRefreshHook, MtlsRefreshTrigger) {
    // Capacity 1: concurrent introspection-POST retries
    // serialise on the channel, which is fine — the actual
    // reload is the slow part and we don't want N concurrent
    // file-reads / fetches anyway.
    let (trigger_tx, trigger_rx) = mpsc::channel(1);
    (
        MtlsRefreshHook {
            inner: Arc::new(MtlsRefreshHookInner { trigger_tx }),
        },
        MtlsRefreshTrigger { trigger_rx },
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[tokio::test]
    async fn force_refresh_returns_source_ok() {
        let (hook, mut trigger) = refresh_channel();

        let task = tokio::spawn(async move {
            if let Some(resp_tx) = trigger.trigger_rx.recv().await {
                let _ = resp_tx.send(Ok(()));
            }
        });

        let result = hook.force_refresh().await;
        assert!(result.is_ok(), "got: {result:?}");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn force_refresh_propagates_source_error() {
        let (hook, mut trigger) = refresh_channel();

        let task = tokio::spawn(async move {
            if let Some(resp_tx) = trigger.trigger_rx.recv().await {
                let _ = resp_tx.send(Err(TakoError::Invalid("PEM parse failed".into())));
            }
        });

        let err = hook.force_refresh().await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("PEM parse failed"), "got: {msg}");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn force_refresh_errors_when_source_dropped() {
        let (hook, trigger) = refresh_channel();
        // Drop the trigger immediately — the watcher task
        // never started, or the watcher has been shut down.
        drop(trigger);

        let err = hook.force_refresh().await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("refresh source dropped"),
            "expected 'refresh source dropped' diagnostic; got: {msg}"
        );
    }

    // The 2-second `REFRESH_TIMEOUT` makes a real-time
    // timeout test slow. The behaviour is structurally
    // straightforward (`tokio::time::timeout(...)` wraps the
    // recv) and is exercised end-to-end by the introspection
    // retry tests in `oidc.rs`. Skipping a dedicated unit
    // test here.
}
