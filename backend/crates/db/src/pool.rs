//! r2d2 + rusqlite pool with the PRAGMA customizer and fresh-DB page_size ordering.
//! See `docs/.tasks/01-db-schema.md` §PRAGMA block.
//!
//! ## PRAGMA ordering (why it is split three ways)
//!
//! `page_size` is a physical property of the database file; SQLite ignores it once
//! any table exists (changing it then requires a full `VACUUM`). So it must be set
//! on a *fresh* file, before the first migration creates a table. We detect
//! freshness by reading `PRAGMA page_count` — a brand-new file reports `0`.
//!
//! `journal_mode = WAL` and `auto_vacuum = INCREMENTAL` are *persisted* on the
//! database and only need to be established once; we (re)apply them when the pool
//! is built so a database restored/copied without them gets fixed up. `auto_vacuum`
//! likewise only takes hold on a fresh DB (or after a `VACUUM`), matching the
//! `page_size` window.
//!
//! The remaining PRAGMAs are *per-connection* runtime settings and must run on
//! every checkout — that is what the [`PragmaCustomizer`] does.

use std::path::Path;

use r2d2::{CustomizeConnection, Pool};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

use crate::DbError;

/// The r2d2 pool type used throughout the crate.
pub type SqlitePool = Pool<SqliteConnectionManager>;
/// A pooled connection handle.
pub type PooledConn = r2d2::PooledConnection<SqliteConnectionManager>;

/// Default pool size when the caller does not override it.
///
/// The `api` crate passes `AppConfig::db_pool_size` (a small multiple of cores).
pub const DEFAULT_POOL_SIZE: u32 = 8;

/// r2d2 customizer that applies the per-connection PRAGMAs on every checkout.
///
/// These are cheap, must run on each connection, and do not persist to the file.
#[derive(Debug, Clone, Copy)]
struct PragmaCustomizer;

/// Per-connection PRAGMAs, applied on every checkout via the customizer.
const PER_CONNECTION_PRAGMAS: &str = "\
    PRAGMA synchronous = NORMAL;
    PRAGMA busy_timeout = 5000;
    PRAGMA foreign_keys = ON;
    PRAGMA cache_size = -262144;
    PRAGMA mmap_size = 1073741824;
    PRAGMA temp_store = MEMORY;
";

impl CustomizeConnection<Connection, rusqlite::Error> for PragmaCustomizer {
    fn on_acquire(&self, conn: &mut Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(PER_CONNECTION_PRAGMAS)
    }
}

/// Build the connection pool for the database at `db_path`.
///
/// This performs the one-time file-level setup on a **single** connection first
/// (fresh-DB `page_size`, then persisted `journal_mode`/`auto_vacuum`), and only
/// then constructs the pool whose connections carry the per-connection PRAGMAs.
///
/// Callers should build the pool, run [`crate::migrate::run`] against it, and then
/// hand it to the rest of the app. All queries must run under
/// `tokio::task::spawn_blocking`.
pub fn build_pool(db_path: impl AsRef<Path>, pool_size: u32) -> Result<SqlitePool, DbError> {
    let db_path = db_path.as_ref();

    // Ensure the parent (/config) exists before SQLite tries to create the file.
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Step 1: file-level setup on a dedicated connection, before the pool exists.
    {
        let conn = Connection::open(db_path)?;
        apply_file_level_pragmas(&conn)?;
    }

    // Step 2: the pool. Every checked-out connection gets the runtime PRAGMAs.
    let manager = SqliteConnectionManager::file(db_path);
    let pool = Pool::builder()
        .max_size(pool_size.max(1))
        .connection_customizer(Box::new(PragmaCustomizer))
        .build(manager)?;

    Ok(pool)
}

/// Apply the settings that are properties of the *file*, in the required order.
///
/// Split out (and public to the crate) so migration tests can assert freshness
/// handling deterministically.
fn apply_file_level_pragmas(conn: &Connection) -> Result<(), DbError> {
    // `page_size` only takes effect on a fresh database (page_count == 0).
    if is_fresh(conn)? {
        // 64 KB pages, aligned to NVMe geometry. Must precede any table creation.
        conn.pragma_update(None, "page_size", 65536)?;
    }

    // Persisted on the database; safe/idempotent to (re)apply. WAL enables
    // concurrent readers with a single writer.
    let journal_mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(DbError::Pragma(format!(
            "journal_mode did not switch to WAL (got {journal_mode:?})"
        )));
    }

    // Incremental auto-vacuum keeps the file from bloating without full VACUUM
    // pauses. Like page_size, only established on a fresh DB.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;

    Ok(())
}

/// A database is "fresh" (never had a table created) when it has zero pages.
fn is_fresh(conn: &Connection) -> Result<bool, DbError> {
    let page_count: i64 = conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
    Ok(page_count == 0)
}
