//! refinery embedded migrations from `backend/migrations/`.
//!
//! The file-level PRAGMAs that must precede table creation (fresh-DB `page_size`,
//! persisted `journal_mode`/`auto_vacuum`) are handled in [`crate::pool`] when the
//! pool is built. By the time [`run`] executes, the connection already has the
//! correct page size, so refinery is free to create tables.
//!
//! refinery records applied versions in its own bookkeeping table
//! (`refinery_schema_history`), which makes `run` idempotent across restarts.

use crate::{pool::SqlitePool, DbError};

// Embeds every `V*.sql` under `backend/migrations/` at compile time.
// Path is relative to this crate's Cargo.toml (`backend/crates/db`).
mod embedded {
    refinery::embed_migrations!("../../migrations");
}

/// Apply all pending migrations against a connection from `pool`.
///
/// Idempotent: on a database already at the latest version this is a no-op.
/// Runs synchronously; callers on the async runtime must wrap it in
/// `tokio::task::spawn_blocking`.
pub fn run(pool: &SqlitePool) -> Result<(), DbError> {
    let mut conn = pool.get()?;
    let report = embedded::migrations::runner().run(&mut *conn)?;

    let applied = report.applied_migrations();
    if applied.is_empty() {
        tracing::debug!("db already at latest migration");
    } else {
        for m in applied {
            tracing::info!(version = m.version(), name = m.name(), "applied migration");
        }
    }

    Ok(())
}
