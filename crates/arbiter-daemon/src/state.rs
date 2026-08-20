use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use arbiter_core::health::DaemonIdentity;
use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AppState {
    pub(crate) provider: Arc<CodexUpstreamProvider>,
    pub(crate) store: SqliteEventStore,
    pub(crate) runtime: RuntimeState,
    pub(crate) identity: DaemonIdentity,
}

impl AppState {
    #[must_use]
    pub fn new(provider: CodexUpstreamProvider, store: SqliteEventStore) -> Self {
        Self::new_with_identity(
            provider,
            store,
            DaemonIdentity {
                pid: std::process::id(),
                port: 0,
                version: env!("CARGO_PKG_VERSION").to_owned(),
                instance_id: uuid::Uuid::new_v4().to_string(),
            },
        )
    }

    #[must_use]
    pub fn new_with_identity(
        provider: CodexUpstreamProvider,
        store: SqliteEventStore,
        identity: DaemonIdentity,
    ) -> Self {
        Self {
            provider: Arc::new(provider),
            store,
            runtime: RuntimeState::default(),
            identity,
        }
    }

    pub(crate) fn start_request(&self) -> Option<RequestActivity> {
        self.runtime.start_request()
    }

    pub(crate) fn begin_shutdown(&self) {
        self.runtime
            .inner
            .shutting_down
            .store(true, Ordering::Release);
    }

    pub(crate) fn force_cancel(&self) {
        self.runtime.inner.force_cancel.cancel();
    }

    pub(crate) async fn wait_for_idle(&self) {
        self.runtime.wait_for_idle().await;
    }

    pub(crate) async fn wait_for_persistence(&self) {
        self.runtime.wait_for_persistence().await;
    }
}

#[derive(Clone, Default)]
pub(crate) struct RuntimeState {
    inner: Arc<RuntimeInner>,
}

#[derive(Default)]
struct RuntimeInner {
    shutting_down: AtomicBool,
    active_requests: AtomicUsize,
    pending_persistence: AtomicUsize,
    idle: Notify,
    persistence_idle: Notify,
    force_cancel: CancellationToken,
}

impl RuntimeState {
    fn start_request(&self) -> Option<RequestActivity> {
        self.inner.active_requests.fetch_add(1, Ordering::AcqRel);
        if self.inner.shutting_down.load(Ordering::Acquire) {
            finish_counter(&self.inner.active_requests, &self.inner.idle);
            None
        } else {
            Some(RequestActivity {
                runtime: self.clone(),
            })
        }
    }

    pub(crate) fn spawn_persistence<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.inner
            .pending_persistence
            .fetch_add(1, Ordering::AcqRel);
        let runtime = self.clone();
        tokio::spawn(async move {
            future.await;
            finish_counter(
                &runtime.inner.pending_persistence,
                &runtime.inner.persistence_idle,
            );
        });
    }

    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.inner.force_cancel.clone()
    }

    async fn wait_for_idle(&self) {
        wait_for_zero(&self.inner.active_requests, &self.inner.idle).await;
    }

    async fn wait_for_persistence(&self) {
        wait_for_zero(
            &self.inner.pending_persistence,
            &self.inner.persistence_idle,
        )
        .await;
    }
}

pub(crate) struct RequestActivity {
    runtime: RuntimeState,
}

impl Drop for RequestActivity {
    fn drop(&mut self) {
        finish_counter(
            &self.runtime.inner.active_requests,
            &self.runtime.inner.idle,
        );
    }
}

fn finish_counter(counter: &AtomicUsize, notify: &Notify) {
    if counter.fetch_sub(1, Ordering::AcqRel) == 1 {
        notify.notify_waiters();
    }
}

async fn wait_for_zero(counter: &AtomicUsize, notify: &Notify) {
    loop {
        let notified = notify.notified();
        if counter.load(Ordering::Acquire) == 0 {
            return;
        }
        notified.await;
    }
}
