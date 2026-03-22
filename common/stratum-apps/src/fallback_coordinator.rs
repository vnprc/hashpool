use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// coordinates fallback operations across multiple components with acknowledgement.
///
/// this is meant to be used together with [`crate::task_manager::TaskManager`],
/// as it allows triggering a fallback event (via [`CancellationToken`]) and waiting
/// until all registered components have completed their cleanup.
///
/// in summary, every time we spawn a fallback-relevant task inside the manager, we MUST:
/// - call [`FallbackCoordinator::register`] at task bootstrap
/// - call [`FallbackCoordinator::done`] at task completion
///
/// when a fallback trigger arrives to the main status loop, we MUST call
/// [`FallbackCoordinator::trigger_and_wait`] to wait for all registered components to complete
/// their cleanup before re-initializing them under the new upstream server.
///
/// finally, a new [`FallbackCoordinator`] must be instantiated for the next fallback cycle.
#[derive(Debug, Clone)]
pub struct FallbackCoordinator {
    signal: CancellationToken,
    pending_tasks: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl Default for FallbackCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl FallbackCoordinator {
    pub fn new() -> Self {
        Self {
            signal: CancellationToken::new(),
            pending_tasks: Arc::new(AtomicUsize::new(0)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// register a component that will participate in fallback coordination
    /// returns a [`FallbackHandler`] that must be called when the component is done
    #[must_use]
    pub fn register(&self) -> FallbackHandler {
        tracing::debug!("FallbackCoordinator: registering component");
        self.pending_tasks.fetch_add(1, Ordering::Relaxed);

        FallbackHandler {
            coordinator: self.clone(),
            done: AtomicBool::new(false),
        }
    }

    /// get the cancellation token that signals fallback
    pub fn token(&self) -> CancellationToken {
        self.signal.clone()
    }

    /// trigger fallback and wait for all registered components to acknowledge
    pub async fn trigger_fallback_and_wait(&self) {
        tracing::debug!("FallbackCoordinator: triggering fallback");
        self.signal.cancel();

        if self.pending_tasks.load(Ordering::Acquire) == 0 {
            return; // all tasks already done
        }

        // there's still some tasks running,
        // wait for the last task to notify us
        self.notify.notified().await;
        tracing::debug!("FallbackCoordinator: finished waiting for components to complete cleanup");
    }
}

pub struct FallbackHandler {
    coordinator: FallbackCoordinator,
    done: AtomicBool,
}

/// Handler for a component that will participate in fallback coordination
///
/// ⚠️ Warning: dropping this handler without calling [`FallbackHandler::done`] will result in a
/// panic.
impl FallbackHandler {
    /// Mark this handler as finished
    /// Takes ownership of `self`, preventing double-calling
    pub fn done(self) {
        tracing::debug!("FallbackHandler: done called");
        self.done.store(true, Ordering::Release);

        let prev = self
            .coordinator
            .pending_tasks
            .fetch_sub(1, Ordering::Release);

        // Notify if fallback has been triggered and this is the last handler
        if self.coordinator.signal.is_cancelled() && prev == 1 {
            self.coordinator.notify.notify_one();
        }
    }
}

impl Drop for FallbackHandler {
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) {
            panic!("FallbackHandler dropped without calling done()");
        }
    }
}
