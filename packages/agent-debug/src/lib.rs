//! Offline Agent Status diagnostics.

pub mod audit;
pub mod cli;
pub mod client;
pub mod comparison;
pub mod database;
pub mod diff;
pub mod discovery;
mod opencode;
pub mod output;
pub mod replay;
pub mod transcript;
pub mod validation;
pub mod verification;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Arguments(clap::Error),
    #[error("background operation failed: {0}")]
    Task(tokio::task::JoinError),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("transcript parser failed: {0}")]
    Parser(#[from] agent_parser::Error),
    #[error("Agent Status database operation failed: {0}")]
    Store(#[from] agent_status_store::Error),
    #[error("SQLite operation failed: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("ingestion failed: {0}")]
    Ingestion(#[from] agent_ingester::ingestion::Error),
    #[error("summarizer initialization failed: {0}")]
    Summarizer(#[from] reqwest::Error),
    #[error("stored JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unable to auto-detect harness: {0}")]
    UnknownHarness(String),
    #[error("{0}")]
    InvalidInput(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Remote(#[from] client::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn check_cancel(cancel: &tokio_util::sync::CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
