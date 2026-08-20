use std::{path::Path, time::Duration};

use arbiter_core::{events::GovernorEvent, ids::AttemptId};
use sqlx::{
    SqlitePool,
    migrate::MigrateError,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;
use tokio::sync::watch;

const CHECKPOINT_IDLE: Duration = Duration::from_millis(250);

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite operation failed: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("SQLite migration failed: {0}")]
    Migration(#[from] MigrateError),
    #[error("event serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("SQLite integrity check returned {0:?}")]
    Integrity(String),
    #[error("event numeric field is outside SQLite's signed integer range")]
    IntegerOutOfRange,
}

impl StoreError {
    #[must_use]
    pub fn is_duplicate_event(&self) -> bool {
        matches!(self, Self::Sqlx(sqlx::Error::Database(error)) if error.is_unique_violation())
    }
}

#[derive(Debug, Clone)]
pub struct SqliteEventStore {
    pool: SqlitePool,
    checkpoint_tx: watch::Sender<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentAttemptCounts {
    pub started: u64,
    pub completed: u64,
    pub failed: u64,
}

impl SqliteEventStore {
    /// Opens or creates an event database, runs migrations, and verifies integrity.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot be opened, migrated, or does not
    /// pass `SQLite`'s integrity check.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .pragma("wal_autocheckpoint", "0")
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        let checkpoint_tx = spawn_idle_checkpoints(&pool);
        let store = Self {
            pool,
            checkpoint_tx,
        };
        store.require_integrity().await?;
        Ok(store)
    }

    /// Appends one immutable event.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization or insertion fails. A duplicate event
    /// ID is reported as a database uniqueness error and never updates history.
    pub async fn append(&self, event: &GovernorEvent) -> Result<(), StoreError> {
        let occurred_at =
            i64::try_from(event.occurred_at_unix_ms).map_err(|_| StoreError::IntegerOutOfRange)?;
        let attempt_index = i64::from(event.attempt_index());
        let payload = serde_json::to_string(event)?;

        sqlx::query(
            "INSERT INTO governor_events (\
                event_id, occurred_at_unix_ms, event_type, request_id, attempt_id, \
                schema_version, payload_json, attempt_index\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event.event_id.to_string())
        .bind(occurred_at)
        .bind(event.event_type())
        .bind(event.request_id().to_string())
        .bind(event.attempt_id().to_string())
        .bind(i64::from(event.schema_version))
        .bind(payload)
        .bind(attempt_index)
        .execute(&self.pool)
        .await?;
        self.checkpoint_tx.send_replace(());

        Ok(())
    }

    /// Reads all events for an attempt in insertion order.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails or a stored event cannot be decoded.
    pub async fn events_for_attempt(
        &self,
        attempt_id: AttemptId,
    ) -> Result<Vec<GovernorEvent>, StoreError> {
        let payloads = sqlx::query_scalar::<_, String>(
            "SELECT payload_json FROM governor_events WHERE attempt_id = ? ORDER BY rowid",
        )
        .bind(attempt_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        payloads
            .into_iter()
            .map(|payload| serde_json::from_str(&payload).map_err(StoreError::from))
            .collect()
    }

    /// Reads the lifecycle events for the most recently started attempt.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails or a stored event cannot be decoded.
    pub async fn latest_attempt_events(&self) -> Result<Vec<GovernorEvent>, StoreError> {
        let payloads = sqlx::query_scalar::<_, String>(
            "SELECT payload_json FROM governor_events \
             WHERE attempt_id = (\
                 SELECT attempt_id FROM governor_events \
                 WHERE event_type = 'attempt_started' ORDER BY rowid DESC LIMIT 1\
             ) ORDER BY rowid",
        )
        .fetch_all(&self.pool)
        .await?;
        payloads
            .into_iter()
            .map(|payload| serde_json::from_str(&payload).map_err(StoreError::from))
            .collect()
    }

    /// Runs `SQLite`'s full database integrity check.
    ///
    /// # Errors
    ///
    /// Returns an error when the integrity query cannot be executed.
    pub async fn integrity_check(&self) -> Result<String, StoreError> {
        sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::from)
    }

    /// Counts attempt lifecycle events at or after a Unix millisecond timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the count query cannot be executed or represented.
    pub async fn recent_attempt_counts(
        &self,
        since_unix_ms: u64,
    ) -> Result<RecentAttemptCounts, StoreError> {
        let since = i64::try_from(since_unix_ms).map_err(|_| StoreError::IntegerOutOfRange)?;
        let (started, completed, failed): (i64, i64, i64) = sqlx::query_as(
            "SELECT \
                COALESCE(SUM(CASE WHEN event_type = 'attempt_started' THEN 1 ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN event_type = 'attempt_completed' THEN 1 ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN event_type = 'attempt_failed' THEN 1 ELSE 0 END), 0) \
             FROM governor_events WHERE occurred_at_unix_ms >= ?",
        )
        .bind(since)
        .fetch_one(&self.pool)
        .await?;
        Ok(RecentAttemptCounts {
            started: u64::try_from(started).map_err(|_| StoreError::IntegerOutOfRange)?,
            completed: u64::try_from(completed).map_err(|_| StoreError::IntegerOutOfRange)?,
            failed: u64::try_from(failed).map_err(|_| StoreError::IntegerOutOfRange)?,
        })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    async fn require_integrity(&self) -> Result<(), StoreError> {
        let result = self.integrity_check().await?;
        if result == "ok" {
            Ok(())
        } else {
            Err(StoreError::Integrity(result))
        }
    }
}

fn spawn_idle_checkpoints(pool: &SqlitePool) -> watch::Sender<()> {
    let pool = pool.clone();
    let (checkpoint_tx, mut checkpoint_rx) = watch::channel(());
    tokio::spawn(async move {
        'maintenance: loop {
            tokio::select! {
                () = pool.close_event() => break 'maintenance,
                changed = checkpoint_rx.changed() => {
                    if changed.is_err() {
                        break 'maintenance;
                    }
                }
            }

            loop {
                tokio::select! {
                    () = pool.close_event() => break 'maintenance,
                    changed = checkpoint_rx.changed() => {
                        if changed.is_err() {
                            break 'maintenance;
                        }
                    }
                    () = tokio::time::sleep(CHECKPOINT_IDLE) => break,
                }
            }

            let _ = sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
                .execute(&pool)
                .await;
        }
    });
    checkpoint_tx
}

#[cfg(test)]
mod tests {
    use arbiter_core::{
        config::BaselineTarget,
        events::{AttemptCompleted, AttemptStarted, GovernorEvent, TokenUsage},
        ids::{AttemptId, RequestId},
    };
    use tempfile::tempdir;

    use super::SqliteEventStore;

    fn attempt_events() -> (AttemptId, GovernorEvent, GovernorEvent) {
        let request_id = RequestId::new();
        let attempt_id = AttemptId::new();
        let target = BaselineTarget::m0();
        let started = GovernorEvent::attempt_started(AttemptStarted {
            request_id,
            attempt_id,
            attempt_index: 0,
            target: target.clone(),
            started_at_unix_ms: 1_000,
        });
        let completed = GovernorEvent::attempt_completed(AttemptCompleted {
            request_id,
            attempt_id,
            attempt_index: 0,
            target,
            started_at_unix_ms: 1_000,
            completed_at_unix_ms: 1_025,
            duration_ms: 25,
            usage: TokenUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                output_tokens: 5,
                reasoning_tokens: 3,
            },
            provider_response_id: Some("resp_123".to_owned()),
        });
        (attempt_id, started, completed)
    }

    #[tokio::test]
    async fn appended_events_are_read_in_order_after_reopen() {
        let temporary = tempdir().expect("temporary directory");
        let database = temporary.path().join("arbiter.db");
        let (attempt_id, started, completed) = attempt_events();

        {
            let store = SqliteEventStore::open(&database).await.expect("open store");
            store.append(&started).await.expect("append start");
            store.append(&completed).await.expect("append completion");
            store.close().await;
        }

        let reopened = SqliteEventStore::open(&database)
            .await
            .expect("reopen store");
        assert_eq!(reopened.integrity_check().await.expect("integrity"), "ok");
        assert_eq!(
            reopened
                .events_for_attempt(attempt_id)
                .await
                .expect("read events"),
            vec![started.clone(), completed.clone()]
        );
        assert_eq!(
            reopened
                .recent_attempt_counts(0)
                .await
                .expect("attempt counts"),
            super::RecentAttemptCounts {
                started: 1,
                completed: 1,
                failed: 0,
            }
        );
        assert_eq!(
            reopened
                .latest_attempt_events()
                .await
                .expect("latest attempt"),
            vec![started, completed]
        );
    }

    #[tokio::test]
    async fn duplicate_event_ids_are_rejected_without_overwriting_history() {
        let temporary = tempdir().expect("temporary directory");
        let database = temporary.path().join("arbiter.db");
        let store = SqliteEventStore::open(&database).await.expect("open store");
        let (attempt_id, started, _) = attempt_events();

        store.append(&started).await.expect("first append");
        let error = store
            .append(&started)
            .await
            .expect_err("duplicate event id must fail");

        assert!(error.is_duplicate_event());
        assert_eq!(
            store
                .events_for_attempt(attempt_id)
                .await
                .expect("read events"),
            vec![started]
        );
    }

    #[tokio::test]
    async fn checkpoints_are_kept_off_request_commits() {
        let temporary = tempdir().expect("temporary directory");
        let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
            .await
            .expect("open store");

        let auto_checkpoint: i64 = sqlx::query_scalar("PRAGMA wal_autocheckpoint")
            .fetch_one(&store.pool)
            .await
            .expect("read WAL checkpoint policy");

        assert_eq!(auto_checkpoint, 0);
    }
}
