//! Durable `SQLite` storage for Arbiter events.

mod store;

pub use store::{SqliteEventStore, StoreError};
