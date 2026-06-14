//! `cover_art` repository — backs the `/imageproxy` route and the
//! cover provider's L1 cache.

use chrono::{DateTime, Utc};
use sqlx::any::Any;
use sqlx::{Pool, Row};

use crate::cover_art::CoverArtRecord;
use crate::error::{StorageError, StorageResult};

type Db = Any;

#[derive(Clone)]
pub struct CoverArtRepository {
    pool: Pool<Any>,
}

impl std::fmt::Debug for CoverArtRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoverArtRepository").finish()
    }
}

impl CoverArtRepository {
    pub fn new(pool: Pool<Any>) -> Self {
        Self { pool }
    }

    /// Look up a cached cover by `(provider, item_id)`. Returns
    /// `None` if the row is missing or the cached blob is empty.
    pub async fn find(
        &self,
        provider: &str,
        item_id: &str,
    ) -> StorageResult<Option<CoverArtRecord>> {
        let row = sqlx::query::<Db>(
            "SELECT image_id, provider, item_id, url, content_type, width, height,
                    bytes, fetched_at, last_used_at
             FROM cover_art WHERE provider = ?1 AND item_id = ?2",
        )
        .bind(provider)
        .bind(item_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(row) => {
                let bytes: Option<Vec<u8>> = row.try_get("bytes").ok();
                if bytes.as_deref().map(|b| b.is_empty()).unwrap_or(true) {
                    return Ok(None);
                }
                let fetched_str: String = row.try_get("fetched_at")?;
                let fetched_at = DateTime::parse_from_rfc3339(&fetched_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let last_used_at = row
                    .try_get::<Option<String>, _>("last_used_at")
                    .ok()
                    .flatten()
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc));
                Ok(Some(CoverArtRecord {
                    image_id: row.try_get("image_id")?,
                    provider: row.try_get("provider")?,
                    item_id: row.try_get("item_id")?,
                    url: row.try_get("url").ok(),
                    content_type: row.try_get("content_type")?,
                    width: row.try_get("width").ok(),
                    height: row.try_get("height").ok(),
                    bytes: bytes.unwrap_or_default(),
                    fetched_at,
                    last_used_at,
                }))
            }
        }
    }

    /// Look up by the opaque `image_id` (the SHA-1 hash the MA
    /// frontend uses as a stable identifier).
    pub async fn find_by_id(&self, image_id: &str) -> StorageResult<Option<CoverArtRecord>> {
        let row = sqlx::query::<Db>(
            "SELECT image_id, provider, item_id, url, content_type, width, height,
                    bytes, fetched_at, last_used_at
             FROM cover_art WHERE image_id = ?1",
        )
        .bind(image_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(row) => {
                let bytes: Option<Vec<u8>> = row.try_get("bytes").ok();
                if bytes.as_deref().map(|b| b.is_empty()).unwrap_or(true) {
                    return Ok(None);
                }
                let fetched_str: String = row.try_get("fetched_at")?;
                let fetched_at = DateTime::parse_from_rfc3339(&fetched_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let last_used_at = row
                    .try_get::<Option<String>, _>("last_used_at")
                    .ok()
                    .flatten()
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc));
                Ok(Some(CoverArtRecord {
                    image_id: row.try_get("image_id")?,
                    provider: row.try_get("provider")?,
                    item_id: row.try_get("item_id")?,
                    url: row.try_get("url").ok(),
                    content_type: row.try_get("content_type")?,
                    width: row.try_get("width").ok(),
                    height: row.try_get("height").ok(),
                    bytes: bytes.unwrap_or_default(),
                    fetched_at,
                    last_used_at,
                }))
            }
        }
    }

    /// Insert or replace a cover-art row.
    pub async fn upsert(&self, record: &CoverArtRecord) -> StorageResult<()> {
        if record.bytes.is_empty() {
            return Err(StorageError::InvalidInput(
                "cover_art row has empty bytes".into(),
            ));
        }
        sqlx::query::<Db>(
            "INSERT INTO cover_art
                (image_id, provider, item_id, url, content_type, width, height,
                 bytes, fetched_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(image_id) DO UPDATE SET
                provider=excluded.provider,
                item_id=excluded.item_id,
                url=excluded.url,
                content_type=excluded.content_type,
                width=excluded.width,
                height=excluded.height,
                bytes=excluded.bytes,
                fetched_at=excluded.fetched_at,
                last_used_at=excluded.last_used_at",
        )
        .bind(&record.image_id)
        .bind(&record.provider)
        .bind(&record.item_id)
        .bind(&record.url)
        .bind(&record.content_type)
        .bind(record.width)
        .bind(record.height)
        .bind(&record.bytes)
        .bind(record.fetched_at.to_rfc3339())
        .bind(record.last_used_at.map(|d| d.to_rfc3339()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a cover by id.
    pub async fn delete(&self, image_id: &str) -> StorageResult<bool> {
        let result = sqlx::query::<Db>("DELETE FROM cover_art WHERE image_id = ?1")
            .bind(image_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Total number of cached covers (debug / metrics).
    pub async fn count(&self) -> StorageResult<i64> {
        let row = sqlx::query::<Db>("SELECT COUNT(*) AS n FROM cover_art")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n").unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{Database, DatabaseConfig};

    async fn repo() -> CoverArtRepository {
        let db = Database::connect(DatabaseConfig::in_memory_sqlite())
            .await
            .unwrap();
        CoverArtRepository::new(db.pool().clone())
    }

    fn rec(id: &str) -> CoverArtRecord {
        CoverArtRecord {
            image_id: id.into(),
            provider: "itunes".into(),
            item_id: format!("track-{id}"),
            url: Some("https://example.com/x.jpg".into()),
            content_type: "image/jpeg".into(),
            width: Some(640),
            height: Some(640),
            bytes: b"\xFF\xD8\xFF\xE0fake".to_vec(),
            fetched_at: Utc::now(),
            last_used_at: None,
        }
    }

    #[tokio::test]
    async fn upsert_then_find() {
        let r = repo().await;
        r.upsert(&rec("abc")).await.unwrap();
        let got = r.find("itunes", "track-abc").await.unwrap().unwrap();
        assert_eq!(got.image_id, "abc");
        assert_eq!(got.width, Some(640));
        assert_eq!(r.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn find_by_id() {
        let r = repo().await;
        r.upsert(&rec("xyz")).await.unwrap();
        let got = r.find_by_id("xyz").await.unwrap().unwrap();
        assert_eq!(got.provider, "itunes");
    }

    #[tokio::test]
    async fn upsert_rejects_empty() {
        let r = repo().await;
        let mut bad = rec("bad");
        bad.bytes.clear();
        assert!(r.upsert(&bad).await.is_err());
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let r = repo().await;
        r.upsert(&rec("del")).await.unwrap();
        assert!(r.delete("del").await.unwrap());
        assert!(!r.delete("del").await.unwrap());
        assert_eq!(r.count().await.unwrap(), 0);
    }
}
