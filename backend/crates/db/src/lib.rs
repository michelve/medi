//! `medi-db` — SQLite persistence layer.
//!
//! Owns the r2d2 connection pool, the per-connection PRAGMA customizer, fresh-DB
//! `page_size` ordering, refinery migrations, and typed query functions.
//! Schema and tuning: `docs/.tasks/01-db-schema.md`.
//!
//! ## Usage
//!
//! ```no_run
//! # use medi_core::AppConfig;
//! let cfg = AppConfig::default();
//! let db = medi_db::open(cfg.db_path(), cfg.db_pool_size).expect("open library.db");
//! // hand `db.pool()` to the api/ingest/assets crates; run queries under
//! // tokio::task::spawn_blocking.
//! ```
//!
//! Every query is synchronous rusqlite; callers on the async runtime MUST wrap
//! calls in `tokio::task::spawn_blocking` (`01-db-schema.md` §Scaling notes).

pub mod migrate;
pub mod models;
pub mod pool;
pub mod queries;
pub mod writes;

pub use pool::{PooledConn, SqlitePool, DEFAULT_POOL_SIZE};

use std::path::Path;

use thiserror::Error;

/// Result alias for this crate.
pub type DbResult<T> = std::result::Result<T, DbError>;

/// Errors from the persistence layer.
#[derive(Debug, Error)]
pub enum DbError {
    /// No row matched (e.g. `get_movie` on an unknown id).
    #[error("not found")]
    NotFound,

    /// A required PRAGMA did not take effect.
    #[error("pragma error: {0}")]
    Pragma(String),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("migration error: {0}")]
    Migration(#[from] refinery::Error),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Map persistence errors onto the shared `medi-core` error so the `api` layer can
/// bubble them with `?`. `NotFound` is preserved as its dedicated variant; every
/// other cause collapses into `Other`.
impl From<DbError> for medi_core::Error {
    fn from(e: DbError) -> Self {
        match e {
            DbError::NotFound => medi_core::Error::NotFound,
            other => medi_core::Error::Other(anyhow::Error::new(other)),
        }
    }
}

/// An opened, migrated database ready for use.
///
/// Cheap to clone — the inner [`SqlitePool`] is an `Arc` under the hood, so clones
/// share the same pool. Hand a clone to each crate that needs the database.
#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// The underlying r2d2 pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Check out a connection from the pool.
    pub fn conn(&self) -> DbResult<PooledConn> {
        Ok(self.pool.get()?)
    }
}

/// Open (creating if absent) the database at `db_path`, apply file-level tuning,
/// build the pool, and run all pending migrations. Returns a ready [`Db`].
///
/// This is the single entry point the rest of the backend uses at boot. It is
/// idempotent across restarts: on an existing, up-to-date database it re-applies
/// only the per-connection/persisted PRAGMAs and finds no migrations to run.
pub fn open(db_path: impl AsRef<Path>, pool_size: u32) -> DbResult<Db> {
    let pool = pool::build_pool(db_path, pool_size)?;
    migrate::run(&pool)?;
    Ok(Db { pool })
}
