//! Database migrations.
//!
//! The `sqlx::migrate!()` macro embeds the SQL files in `migrations/`
//! at compile time and runs them at startup. Migrations are
//! append-only: never edit a file once it's been applied to a
//! production database; add a new one instead.

use sqlx::any::Any;
use sqlx::migrate;
use sqlx::Pool;

use crate::error::StorageResult;

pub async fn run(pool: &Pool<Any>) -> StorageResult<()> {
    let migrator = migrate!("./migrations");
    migrator.run(pool).await?;
    Ok(())
}
