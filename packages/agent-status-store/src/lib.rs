//! SQLite schema and domain queries shared by the ingester, server, and offline debugger.
//! Callers own native `rusqlite::Connection` and `Transaction` values.
#![doc = include_str!("../README.md")]

pub mod read;
pub mod schema;
pub mod write;

use rusqlite::{Connection, OpenFlags};
use std::{path::Path, time::Duration};

pub fn open_writer(path: impl AsRef<Path>) -> Result<Connection> {
    let path = path.as_ref();
    if path != Path::new(":memory:")
        && let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA auto_vacuum=INCREMENTAL;")?;
    schema::migrate(&mut db)?;
    Ok(db)
}

pub fn open_reader(path: impl AsRef<Path>) -> Result<Connection> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.pragma_update(None, "foreign_keys", true)?;
    Ok(db)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("SQLite operation failed")]
    Sql(#[from] rusqlite::Error),
    #[error("stored telemetry JSON is invalid")]
    Json(#[from] serde_json::Error),
    #[error("database filesystem operation failed")]
    Io(#[from] std::io::Error),
    #[error("agent not found")]
    NotFound,
    #[error("unsupported database schema version {0}")]
    SchemaVersion(i64),
    #[error("foreign key check failed for table {table}, parent {parent}, row {rowid:?}")]
    ForeignKey {
        table: String,
        parent: String,
        rowid: Option<i64>,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
