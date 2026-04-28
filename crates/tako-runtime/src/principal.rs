//! Task-local `Principal` propagation.
//!
//! Set the principal at the entry point of a request and any descendant
//! Tokio task can read it back via [`current`]. Useful for OTel span
//! attributes, audit logs, and policy contexts that don't carry the
//! Principal explicitly.

use tako_core::Principal;

tokio::task_local! {
    static CURRENT: Principal;
}

/// Run `fut` with `principal` bound as the task-local current principal.
pub async fn with_principal<F: std::future::Future>(principal: Principal, fut: F) -> F::Output {
    CURRENT.scope(principal, fut).await
}

/// Returns the principal bound to the current task, if any.
///
/// Returns `None` if called outside a `with_principal` scope. Callers that
/// require a principal should propagate one explicitly through their API
/// rather than relying on this fallback.
pub fn current() -> Option<Principal> {
    CURRENT.try_with(|p| p.clone()).ok()
}
