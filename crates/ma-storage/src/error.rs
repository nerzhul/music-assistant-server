//! Storage error type. Translates `sqlx::Error` into our domain.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migration(String),
    #[error("migrate error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("query helper error: {0}")]
    Query(String),
}

pub type StorageResult<T> = std::result::Result<T, StorageError>;
