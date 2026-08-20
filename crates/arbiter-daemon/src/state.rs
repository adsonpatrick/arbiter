use std::sync::Arc;

use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;

#[derive(Clone)]
pub struct AppState {
    pub(crate) provider: Arc<CodexUpstreamProvider>,
    pub(crate) store: SqliteEventStore,
}

impl AppState {
    #[must_use]
    pub fn new(provider: CodexUpstreamProvider, store: SqliteEventStore) -> Self {
        Self {
            provider: Arc::new(provider),
            store,
        }
    }
}
