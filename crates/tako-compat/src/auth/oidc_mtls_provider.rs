//! Phase 37 — trait-based mTLS identity provider for
//! proactive expiry-driven cert refresh.
//!
//! Complements the Phase 35 filesystem watcher
//! ([`crate::auth::oidc_mtls_watcher`]) for deployments where
//! the cert+key don't live on disk:
//!
//! - **HSM-backed keys** — the private key never leaves the
//!   HSM; cert is rotated via vendor SDK.
//! - **In-memory secret stores** — operator's app fetches
//!   cert+key from a vault/broker on demand.
//! - **On-demand identity brokers** — SPIFFE Workload API,
//!   AWS IAM Roles Anywhere, etc.
//!
//! Operators implement [`MtlsIdentityProvider::fetch`]; tako
//! parses the returned cert's `NotAfter` and proactively
//! re-calls fetch at 80% of the validity window. The
//! previously installed `MtlsClient` is preserved if a fetch
//! fails or returns malformed PEM.

use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use tako_core::TakoError;
use tokio::{sync::Notify, task::JoinHandle, time::Instant};

use crate::auth::oidc::OidcAuthResolver;
use crate::auth::oidc_mtls_hook::{MtlsRefreshHook, MtlsRefreshTrigger, refresh_channel};

/// Phase 37 — fraction of the cert's validity window after
/// which to refresh. 0.8 leaves a 20% buffer; matches industry
/// convention (cert-manager, SPIRE workload SVIDs).
const REFRESH_FRACTION: f64 = 0.8;

/// Phase 37 — fallback refresh interval when the cert can't be
/// parsed (operator returned a blob the trait pretended was a
/// cert; the reload itself succeeded so it's not a hard error).
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

/// Phase 37 — cap on the refresh interval. A cert with 100-year
/// validity shouldn't sleep for 80 years and miss out-of-band
/// rotation signals (e.g. the operator's vault just gave us a
/// fresh cert; we want to pick it up within a day even if the
/// old one is "still valid for decades").
const MAX_REFRESH_INTERVAL: Duration = Duration::from_secs(86400);

/// Phase 37 — backoff after a failed fetch / failed reload.
const ERROR_BACKOFF: Duration = Duration::from_secs(60);

/// Phase 37 — PEM-pair returned by
/// [`MtlsIdentityProvider::fetch`].
///
/// Fields are owned `Vec<u8>` so providers can return
/// freshly-allocated bytes per call without lifetime
/// entanglement (HSM SDKs typically own their own buffers).
#[derive(Clone, Debug)]
pub struct MtlsIdentity {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

/// Phase 37 — async source of fresh mTLS cert+key bytes.
///
/// Implement this trait when filesystem-based rotation
/// ([`OidcAuthResolver::watch_mtls_files`], Phase 35) doesn't
/// fit your deployment shape — e.g. HSM-backed keys, in-memory
/// secret stores, on-demand fetch from a SPIFFE / AWS Roles
/// Anywhere broker.
///
/// Tako parses the returned cert's `NotAfter` and proactively
/// re-calls `fetch()` at 80% of the validity window. The
/// previously installed `MtlsClient` is preserved if a fetch
/// fails or returns malformed PEM.
#[async_trait]
pub trait MtlsIdentityProvider: Send + Sync + 'static + std::fmt::Debug {
    /// Return a fresh (cert, key) PEM pair. Called eagerly in
    /// the background once
    /// [`OidcAuthResolver::watch_mtls_provider`] is invoked,
    /// then periodically based on the cert's parsed expiry.
    async fn fetch(&self) -> Result<MtlsIdentity, TakoError>;
}

/// Handle for an active trait-based mTLS identity provider
/// watcher. The background tokio task runs until this handle
/// is dropped (or [`shutdown`](Self::shutdown) is called
/// explicitly). Hold it for the lifetime of the resolver —
/// typically a module-scope variable in production.
pub struct MtlsProviderWatcher {
    shutdown: Arc<Notify>,
    join: Option<JoinHandle<()>>,
    /// Phase 39 — handle the operator passes to
    /// [`OidcAuthResolver::with_mtls_refresh_hook`] to enable
    /// auto-retry of failed introspection POSTs against this
    /// provider's reload primitive.
    refresh_hook: MtlsRefreshHook,
}

impl std::fmt::Debug for MtlsProviderWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtlsProviderWatcher")
            .field(
                "running",
                &self.join.as_ref().is_some_and(|h| !h.is_finished()),
            )
            .finish()
    }
}

impl MtlsProviderWatcher {
    /// Phase 39 — return a `MtlsRefreshHook` wired to this
    /// provider's background task. Pair with
    /// [`OidcAuthResolver::with_mtls_refresh_hook`](crate::auth::oidc::OidcAuthResolver::with_mtls_refresh_hook)
    /// to enable auto-retry of failed introspection POSTs.
    /// The hook is `Clone`-able; multiple resolvers can share
    /// the same refresh source.
    pub fn refresh_hook(&self) -> MtlsRefreshHook {
        self.refresh_hook.clone()
    }

    /// Stop the watcher and await the background task's
    /// teardown. Idempotent — calling twice is a no-op the
    /// second time.
    pub async fn shutdown(mut self) {
        self.shutdown.notify_one();
        if let Some(h) = self.join.take() {
            h.abort();
            let _ = h.await;
        }
    }
}

impl Drop for MtlsProviderWatcher {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        if let Some(h) = self.join.take() {
            h.abort();
        }
    }
}

impl OidcAuthResolver {
    /// Phase 37 — spawn a background task that periodically
    /// fetches a fresh mTLS identity from the operator-supplied
    /// [`MtlsIdentityProvider`] and reloads it via
    /// [`Self::reload_mtls_identity`]. The refresh schedule is
    /// driven by the returned cert's parsed `NotAfter`: tako
    /// sleeps until 80% of the validity window has elapsed,
    /// then re-fetches.
    ///
    /// The returned [`MtlsProviderWatcher`] handle owns the
    /// background task; dropping it (or calling
    /// [`MtlsProviderWatcher::shutdown`]) stops the watcher.
    /// Operators bind it to a module-scope variable for the
    /// lifetime of the resolver.
    ///
    /// Errors:
    ///
    /// - No prior `with_introspection_mtls` /
    ///   `with_introspection_self_signed_mtls` call —
    ///   `TakoError::Invalid` with operator guidance.
    pub fn watch_mtls_provider(
        self: Arc<Self>,
        provider: Arc<dyn MtlsIdentityProvider>,
    ) -> Result<MtlsProviderWatcher, TakoError> {
        if !self.introspection_mtls_configured() {
            return Err(TakoError::Invalid(
                "oidc: watch_mtls_provider called but no mTLS identity configured \
                 (call with_introspection_mtls or with_introspection_self_signed_mtls first)"
                    .into(),
            ));
        }

        let shutdown = Arc::new(Notify::new());
        let task_shutdown = shutdown.clone();
        let resolver = self.clone();

        // Phase 39 — refresh-hook channel. The hook is
        // returned to the operator via
        // `MtlsProviderWatcher::refresh_hook`; the trigger's
        // receiver is consumed by the provider loop's select arm.
        let (refresh_hook, refresh_trigger) = refresh_channel();

        let join = tokio::spawn(async move {
            run_provider_loop(resolver, provider, task_shutdown, refresh_trigger).await;
        });

        Ok(MtlsProviderWatcher {
            shutdown,
            join: Some(join),
            refresh_hook,
        })
    }
}

async fn run_provider_loop(
    resolver: Arc<OidcAuthResolver>,
    provider: Arc<dyn MtlsIdentityProvider>,
    shutdown: Arc<Notify>,
    mut refresh_trigger: MtlsRefreshTrigger,
) {
    loop {
        let (next_sleep, _initial_result) = fetch_and_reload(&provider, &resolver).await;
        let deadline = Instant::now() + next_sleep;
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                tracing::debug!("oidc.mtls_provider: shutdown received");
                return;
            }
            // Phase 39 — refresh-hook trigger. Skip the
            // remaining sleep, refresh on demand, and signal
            // the result back to the introspection-POST retry
            // layer that's waiting on us.
            Some(resp_tx) = refresh_trigger.trigger_rx.recv() => {
                tracing::info!(
                    "oidc.mtls_provider: refresh hook triggered; fetching on demand"
                );
                let (_, result) = fetch_and_reload(&provider, &resolver).await;
                let _ = resp_tx.send(result);
            }
            _ = tokio::time::sleep_until(deadline) => {}
        }
    }
}

/// Phase 39 — single fetch-and-reload step, factored out so the
/// scheduled tick (proactive refresh, Phase 37 cadence) and the
/// refresh-hook trigger (reactive on TLS handshake failure,
/// Phase 39) share the same body. Returns the next sleep
/// interval AND the reload result the hook signals back to its
/// caller.
async fn fetch_and_reload(
    provider: &Arc<dyn MtlsIdentityProvider>,
    resolver: &Arc<OidcAuthResolver>,
) -> (Duration, Result<(), TakoError>) {
    match provider.fetch().await {
        Ok(identity) => {
            match resolver.reload_mtls_identity(&identity.cert_pem, &identity.key_pem) {
                Ok(()) => {
                    let interval = next_refresh_interval(&identity.cert_pem);
                    tracing::info!(
                        next_refresh_secs = interval.as_secs(),
                        "oidc.mtls_provider: refreshed identity from provider"
                    );
                    (interval, Ok(()))
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "oidc.mtls_provider: reload_mtls_identity failed; previous client preserved"
                    );
                    (ERROR_BACKOFF, Err(e))
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "oidc.mtls_provider: provider.fetch() failed; will retry"
            );
            let cloned = TakoError::Invalid(format!("{e}"));
            (ERROR_BACKOFF, Err(cloned))
        }
    }
}

/// Phase 37 — extract the leaf cert's `NotAfter` and compute
/// the refresh interval as `(NotAfter - now) * REFRESH_FRACTION`
/// clamped to `[ERROR_BACKOFF, MAX_REFRESH_INTERVAL]`. Falls
/// back to `DEFAULT_REFRESH_INTERVAL` if the cert can't be
/// parsed or already expired.
fn next_refresh_interval(cert_pem: &[u8]) -> Duration {
    let Some(not_after) = parse_not_after(cert_pem) else {
        tracing::warn!(
            "oidc.mtls_provider: could not parse cert NotAfter; \
             falling back to default refresh interval"
        );
        return DEFAULT_REFRESH_INTERVAL;
    };

    let Ok(remaining) = not_after.duration_since(std::time::SystemTime::now()) else {
        // Cert is already expired (or system clock is wildly skewed).
        // Refresh on the error-backoff cadence so the operator's
        // provider gets a chance to issue a fresh cert quickly.
        tracing::warn!(
            "oidc.mtls_provider: cert NotAfter is in the past; \
             scheduling immediate retry on error-backoff cadence"
        );
        return ERROR_BACKOFF;
    };

    let scaled = remaining.as_secs_f64() * REFRESH_FRACTION;
    let scaled = if scaled.is_finite() && scaled > 0.0 {
        Duration::from_secs_f64(scaled)
    } else {
        DEFAULT_REFRESH_INTERVAL
    };

    scaled.clamp(ERROR_BACKOFF, MAX_REFRESH_INTERVAL)
}

/// Phase 37 — parse the leaf cert's `NotAfter` from a PEM
/// blob. Returns `None` if the cert is unparseable.
fn parse_not_after(cert_pem: &[u8]) -> Option<std::time::SystemTime> {
    use x509_parser::pem::Pem;

    // Walk PEM frames; take the first cert. Multi-cert PEM
    // blobs (cert chains) are common — only the leaf matters
    // for NotAfter, and the leaf is conventionally first.
    let pem = Pem::iter_from_buffer(cert_pem).next()?.ok()?;
    let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents).ok()?;
    let not_after_ts = cert.validity().not_after.timestamp();
    if not_after_ts < 0 {
        return None;
    }
    Some(std::time::UNIX_EPOCH + Duration::from_secs(not_after_ts as u64))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[derive(Debug)]
    struct CountingProvider {
        calls: AtomicUsize,
        result: Mutex<Result<MtlsIdentity, TakoError>>,
    }

    impl CountingProvider {
        fn new(result: Result<MtlsIdentity, TakoError>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                result: Mutex::new(result),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl MtlsIdentityProvider for CountingProvider {
        async fn fetch(&self) -> Result<MtlsIdentity, TakoError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let guard = self.result.lock().expect("test mutex");
            match &*guard {
                Ok(id) => Ok(id.clone()),
                Err(TakoError::Transport(msg)) => Err(TakoError::Transport(msg.clone())),
                Err(TakoError::Invalid(msg)) => Err(TakoError::Invalid(msg.clone())),
                Err(e) => Err(TakoError::Invalid(format!("{e:?}"))),
            }
        }
    }

    fn current_client_ptr(resolver: &OidcAuthResolver) -> *const reqwest::Client {
        let arc = resolver
            .introspection_mtls_client_arc()
            .expect("mTLS configured");
        Arc::as_ptr(&arc.current())
    }

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

    #[test]
    fn parse_not_after_extracts_expiry_from_test_cert() {
        let parsed = parse_not_after(TEST_MTLS_CERT_PEM).expect("test cert NotAfter parses");
        // The test cert has ~100 year validity (not_before
        // 2026-05-01, not_after 2126-04-07). Parsed timestamp
        // should be in the future.
        assert!(parsed > std::time::SystemTime::now());
    }

    #[test]
    fn parse_not_after_returns_none_on_garbage() {
        assert!(parse_not_after(b"not a pem").is_none());
        assert!(
            parse_not_after(b"-----BEGIN CERTIFICATE-----\nGARBAGE\n-----END CERTIFICATE-----\n")
                .is_none()
        );
    }

    #[test]
    fn next_refresh_interval_for_long_lived_cert_is_capped() {
        let interval = next_refresh_interval(TEST_MTLS_CERT_PEM);
        // 100-year cert at 80% would be ~80 years. Cap is 24h.
        assert_eq!(interval, MAX_REFRESH_INTERVAL);
    }

    #[test]
    fn next_refresh_interval_for_garbage_falls_back_to_default() {
        let interval = next_refresh_interval(b"not a pem");
        assert_eq!(interval, DEFAULT_REFRESH_INTERVAL);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provider_fetch_drives_initial_reload() {
        let resolver = Arc::new(build_resolver_with_mtls());
        let initial = current_client_ptr(&resolver);

        let provider = CountingProvider::new(Ok(MtlsIdentity {
            cert_pem: TEST_MTLS_CERT_PEM.to_vec(),
            key_pem: TEST_MTLS_KEY_PEM.to_vec(),
        }));
        let _watcher = resolver
            .clone()
            .watch_mtls_provider(provider.clone() as Arc<dyn MtlsIdentityProvider>)
            .expect("watcher constructs");

        let swapped = wait_for(Duration::from_secs(2), || {
            current_client_ptr(&resolver) != initial
        })
        .await;
        assert!(swapped, "expected initial fetch to swap MtlsClient");
        assert!(provider.call_count() >= 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provider_fetch_error_preserves_client() {
        let resolver = Arc::new(build_resolver_with_mtls());
        let initial = current_client_ptr(&resolver);

        let provider =
            CountingProvider::new(Err(TakoError::Transport("broker unreachable".into())));
        let _watcher = resolver
            .clone()
            .watch_mtls_provider(provider.clone() as Arc<dyn MtlsIdentityProvider>)
            .expect("watcher constructs");

        // Wait long enough for at least one fetch; assert no
        // swap happened.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            current_client_ptr(&resolver),
            initial,
            "client must not swap when fetch errors"
        );
        assert!(provider.call_count() >= 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_stops_watcher() {
        let resolver = Arc::new(build_resolver_with_mtls());
        let provider = CountingProvider::new(Ok(MtlsIdentity {
            cert_pem: TEST_MTLS_CERT_PEM.to_vec(),
            key_pem: TEST_MTLS_KEY_PEM.to_vec(),
        }));
        {
            let _w = resolver
                .clone()
                .watch_mtls_provider(provider.clone() as Arc<dyn MtlsIdentityProvider>)
                .expect("watcher constructs");
            // Drop scope ends here.
        }
        // Watcher is gone; fetch counter should freeze. Wait
        // and verify it doesn't keep ticking.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let frozen = provider.call_count();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            provider.call_count(),
            frozen,
            "dropped watcher must stop calling fetch"
        );
    }

    #[tokio::test]
    async fn errors_when_no_mtls_configured() {
        let resolver = Arc::new(OidcAuthResolver::new_for_internal_testing(
            "https://issuer.example",
            "test-aud",
            Some("https://issuer.example/introspect"),
        ));
        let provider = CountingProvider::new(Ok(MtlsIdentity {
            cert_pem: TEST_MTLS_CERT_PEM.to_vec(),
            key_pem: TEST_MTLS_KEY_PEM.to_vec(),
        }));
        let err = resolver
            .watch_mtls_provider(provider as Arc<dyn MtlsIdentityProvider>)
            .expect_err("should fail without mTLS config");
        let msg = format!("{err}");
        assert!(
            msg.contains("no mTLS identity configured"),
            "unexpected error: {msg}"
        );
    }
}
