//! Connection-pool factory + lifecycle.
//!
//! Supports two backends:
//!
//! * `sqlite://<path>` (e.g. `sqlite::memory:`, `sqlite:///var/lib/ma.db`)
//! * `postgres://user:pass@host:port/dbname` (or `postgresql://`)
//!
//! The backend is detected from the URL scheme at runtime, then a
//! `sqlx::AnyPool` is opened. The `Any` driver dispatches queries to
//! the right concrete driver (sqlite / postgres) and lets us share
//! one pool type across the codebase.

use std::path::Path;
use std::time::Duration;

use sqlx::any::{install_default_drivers, Any, AnyPoolOptions};
use sqlx::ConnectOptions;
use sqlx::Pool;
use tracing::info;
use url::Url;

use crate::error::{StorageError, StorageResult};

/// What kind of database we're connected to. Mostly useful for
/// backends that need to format SQL or types differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    Sqlite,
    Postgres,
}

impl DatabaseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }
}

/// Connection / pool configuration. The `url` field is mandatory and
/// must start with `sqlite://` or `postgres://`.
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout: Duration,
    pub idle_timeout: Option<Duration>,
    pub slow_query_warn_ms: u64,
}

impl DatabaseConfig {
    /// Build a config from envvars.
    ///
    /// * `MA_DATABASE_URL` (or `DATABASE_URL`) — connection string
    /// * `MA_DATABASE_MAX_CONNECTIONS` — pool size (default 8)
    /// * `MA_DATABASE_CONNECT_TIMEOUT_SECS` — connect timeout (default 5s)
    /// * `MA_DATABASE_IDLE_TIMEOUT_SECS` — idle timeout (default 600s)
    pub fn from_env() -> StorageResult<Self> {
        let url = std::env::var("MA_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| "sqlite://music-assistant.db".to_string());
        Self::from_url(&url)
    }

    /// Build a config from an explicit URL, applying the rest of the
    /// config from envvars.
    pub fn from_url(url: &str) -> StorageResult<Self> {
        let max_connections = std::env::var("MA_DATABASE_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        let min_connections = std::env::var("MA_DATABASE_MIN_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let connect_timeout = std::env::var("MA_DATABASE_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);
        let idle_timeout = std::env::var("MA_DATABASE_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok());
        let slow_query_warn_ms = std::env::var("MA_DATABASE_SLOW_QUERY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        Ok(Self {
            url: url.to_string(),
            max_connections,
            min_connections,
            connect_timeout: Duration::from_secs(connect_timeout),
            idle_timeout: idle_timeout.map(Duration::from_secs),
            slow_query_warn_ms,
        })
    }

    /// In-memory SQLite. Useful for tests.
    pub fn in_memory_sqlite() -> Self {
        Self {
            url: "sqlite::memory:".to_string(),
            max_connections: 1,
            min_connections: 1,
            connect_timeout: Duration::from_secs(5),
            idle_timeout: None,
            slow_query_warn_ms: 200,
        }
    }

    /// Auto-derive a SQLite file path from a directory + filename.
    /// The directory is created if missing.
    pub fn sqlite_file(dir: &Path, filename: &str) -> StorageResult<Self> {
        std::fs::create_dir_all(dir).map_err(StorageError::Io)?;
        let path = dir.join(filename);
        // sqlx's `SqliteConnectOptions::from_str` accepts the form
        // `sqlite:///<abs_path>` for absolute paths.
        let url = format!("sqlite://{}", path.display());
        Self::from_url(&url)
    }

    /// Return the backend kind for this URL. Fails if the scheme is
    /// neither `sqlite` / `file` (on-disk) nor `postgres`.
    pub fn kind(&self) -> StorageResult<DatabaseKind> {
        let url = Url::parse(&self.url).map_err(|e| StorageError::Backend(e.to_string()))?;
        match url.scheme() {
            "sqlite" | "file" => Ok(DatabaseKind::Sqlite),
            "postgres" | "postgresql" => Ok(DatabaseKind::Postgres),
            other => Err(StorageError::Backend(format!(
                "unsupported database scheme: {other} (expected sqlite:// or postgres://)"
            ))),
        }
    }
}

/// The runtime database handle. Cheap to clone (`Arc` internally).
#[derive(Clone)]
pub struct Database {
    pool: Pool<Any>,
    kind: DatabaseKind,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("kind", &self.kind)
            .field("size", &self.pool.size())
            .finish()
    }
}

impl Database {
    /// Open a connection pool from a `DatabaseConfig` and run the
    /// embedded migrations.
    pub async fn connect(config: DatabaseConfig) -> StorageResult<Self> {
        // Install the runtime drivers for whichever backends we
        // compiled in. Calling this multiple times is a no-op
        // (sqlx guards with an internal OnceCell).
        install_default_drivers();
        let kind = config.kind()?;

        let mut pool_opts = AnyPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(config.connect_timeout);
        if let Some(idle) = config.idle_timeout {
            pool_opts = pool_opts.idle_timeout(idle);
        }

        // Open the pool by URL — sqlx's `AnyPool` routes to the right
        // driver based on the URL scheme. We post-process the URL
        // only for SQLite (so `sqlite::memory:` and `sqlite:foo.db`
        // both work).
        // For SQLite, we need `create_if_missing=true` so a new
        // on-disk file is created. The driver accepts a `?mode=rwc`
        // parameter that does the same. We inject it once on the
        // user's URL, after the existing query string (if any).
        let mut config = config;
        if matches!(kind, DatabaseKind::Sqlite)
            && !config.url.contains("mode=")
            && !config.url.contains("create_if_missing")
        {
            let sep = if config.url.contains('?') { '&' } else { '?' };
            config.url = format!("{}{}mode=rwc", config.url, sep);
        }
        let url_parsed =
            ::url::Url::parse(&config.url).map_err(|e| StorageError::Backend(e.to_string()))?;
        let connect_opts = sqlx::any::AnyConnectOptions::from_url(&url_parsed)
            .map_err(|e| StorageError::Backend(e.to_string()))?;
        // Apply per-backend defaults that aren't available via the URL.
        // We do this by opening with the URL first, then running
        // backend-specific PRAGMAs / session settings on the first
        // connection.
        let pool = pool_opts.connect_with(connect_opts).await?;
        match kind {
            DatabaseKind::Sqlite => {
                sqlx::query("PRAGMA journal_mode = WAL;")
                    .execute(&pool)
                    .await
                    .ok();
                sqlx::query("PRAGMA synchronous = NORMAL;")
                    .execute(&pool)
                    .await
                    .ok();
                sqlx::query("PRAGMA foreign_keys = ON;")
                    .execute(&pool)
                    .await
                    .ok();
            }
            DatabaseKind::Postgres => {
                sqlx::query("SET application_name = 'music-assistant-rust'")
                    .execute(&pool)
                    .await
                    .ok();
            }
        }

        info!(
            kind = kind.as_str(),
            max = config.max_connections,
            "database pool ready"
        );

        let db = Self { pool, kind };
        db.run_migrations().await?;
        Ok(db)
    }

    /// Run the embedded migration set. Idempotent: sqlx records the
    /// applied version in `_sqlx_migrations`.
    pub async fn run_migrations(&self) -> StorageResult<()> {
        crate::migrations::run(&self.pool).await
    }

    /// Acquire a connection from the pool. The connection is released
    /// when the returned value is dropped.
    pub async fn acquire(&self) -> StorageResult<sqlx::pool::PoolConnection<Any>> {
        Ok(self.pool.acquire().await?)
    }

    /// Convenience: open a transaction and run `f` inside it.
    pub async fn transaction<F, T>(&self, f: F) -> StorageResult<T>
    where
        F: for<'c> FnOnce(
            &'c mut sqlx::Transaction<'_, Any>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = StorageResult<T>> + Send + 'c>,
        >,
    {
        let mut tx = self.pool.begin().await?;
        let out = f(&mut tx).await?;
        tx.commit().await?;
        Ok(out)
    }

    /// Direct pool access for advanced callers.
    pub fn pool(&self) -> &Pool<Any> {
        &self.pool
    }

    pub fn kind(&self) -> DatabaseKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sqlite_url() {
        let cfg = DatabaseConfig::from_url("sqlite::memory:").unwrap();
        assert_eq!(cfg.kind().unwrap(), DatabaseKind::Sqlite);
        let cfg = DatabaseConfig::from_url("sqlite:///tmp/foo.db").unwrap();
        assert_eq!(cfg.kind().unwrap(), DatabaseKind::Sqlite);
    }

    #[test]
    fn detects_postgres_url() {
        let cfg = DatabaseConfig::from_url("postgres://u:p@h:5432/db").unwrap();
        assert_eq!(cfg.kind().unwrap(), DatabaseKind::Postgres);
        let cfg = DatabaseConfig::from_url("postgresql://u@h/db").unwrap();
        assert_eq!(cfg.kind().unwrap(), DatabaseKind::Postgres);
    }

    #[test]
    fn rejects_unknown_scheme() {
        let cfg = DatabaseConfig::from_url("mysql://u@h/db").unwrap();
        assert!(cfg.kind().is_err());
    }

    #[tokio::test]
    async fn in_memory_sqlite_round_trip() {
        let db = Database::connect(DatabaseConfig::in_memory_sqlite())
            .await
            .unwrap();
        // We expect the migration to have created the tables.
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(row.0 > 0);
    }

    #[tokio::test]
    async fn sqlite_file_db_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        eprintln!("tempdir: {:?}", dir.path());
        eprintln!("exists: {}", dir.path().exists());
        let cfg = DatabaseConfig::sqlite_file(dir.path(), "test.db").unwrap();
        eprintln!("URL: {}", cfg.url);
        // Manually open via the SQLite driver to see the real error.
        use std::str::FromStr;
        let opts = sqlx::sqlite::SqliteConnectOptions::from_str(&cfg.url)
            .map_err(|e| format!("from_str: {e}"))
            .unwrap();
        eprintln!("filename: {:?}", opts);
        let db = Database::connect(cfg).await.unwrap();
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(row.0 > 0);
    }
}
