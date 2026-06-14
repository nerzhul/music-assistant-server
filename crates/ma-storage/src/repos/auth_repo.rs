//! Auth repository — `users` and `auth_tokens` tables.
//!
//! Mirrors the in-memory `AuthManager` in `ma-server`; when the
//! storage layer is wired in, all reads / writes go through here.

use chrono::{DateTime, Utc};
use sqlx::any::Any;
use sqlx::{Pool, Row};

use ma_core::auth::{AuthToken, User, UserRole};

use crate::error::{StorageError, StorageResult};

/// Database type used by all queries in this repo. Pinning to
/// `sqlx::Any` lets us write `sqlx::query::<Db>(...)` and have the
/// compiler reject mismatches with the executor's `Database` assoc
/// type.
type Db = Any;

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub user: User,
    pub password_hash: String,
}

#[derive(Clone)]
pub struct AuthRepository {
    pool: Pool<Any>,
}

impl std::fmt::Debug for AuthRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRepository").finish()
    }
}

impl AuthRepository {
    pub fn new(pool: Pool<Any>) -> Self {
        Self { pool }
    }

    /// `true` if the `users` table is non-empty.
    pub async fn has_users(&self) -> StorageResult<bool> {
        let row = sqlx::query::<Db>("SELECT COUNT(*) AS n FROM users")
            .fetch_one(&self.pool)
            .await?;
        let n: i64 = row.try_get("n").unwrap_or(0);
        Ok(n > 0)
    }

    /// Insert or replace a user. The password hash is stored
    /// separately (column `password_hash`) so we can rotate it
    /// without rewriting the rest of the row.
    pub async fn upsert_user(&self, user: &User, password_hash: &str) -> StorageResult<()> {
        let role = user.role.as_str();
        let prefs = serde_json::to_string(&user.preferences)?;
        let provider_filter = serde_json::to_string(&user.provider_filter)?;
        let player_filter = serde_json::to_string(&user.player_filter)?;
        let created_at = user.created_at.to_rfc3339();
        sqlx::query::<Db>(
            "INSERT INTO users (user_id, username, role, enabled, display_name, avatar_url,
                                password_hash, preferences, provider_filter, player_filter, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(user_id) DO UPDATE SET
                username=excluded.username,
                role=excluded.role,
                enabled=excluded.enabled,
                display_name=excluded.display_name,
                avatar_url=excluded.avatar_url,
                password_hash=excluded.password_hash,
                preferences=excluded.preferences,
                provider_filter=excluded.provider_filter,
                player_filter=excluded.player_filter",
        )
        .bind(&user.user_id)
        .bind(&user.username)
        .bind(role)
        .bind(user.enabled)
        .bind(&user.display_name)
        .bind(&user.avatar_url)
        .bind(password_hash)
        .bind(&prefs)
        .bind(&provider_filter)
        .bind(&player_filter)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update only the password hash (used when a known user changes
    /// their password without rewriting the rest of the row).
    pub async fn set_password_hash(&self, user_id: &str, password_hash: &str) -> StorageResult<()> {
        sqlx::query::<Db>("UPDATE users SET password_hash = ?1 WHERE user_id = ?2")
            .bind(password_hash)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Fetch a user + their password hash by username.
    pub async fn find_by_username(&self, username: &str) -> StorageResult<Option<UserRecord>> {
        let row = sqlx::query::<Db>(
            "SELECT user_id, username, role, enabled, display_name, avatar_url,
                    password_hash, preferences, provider_filter, player_filter, created_at
             FROM users WHERE username = ?1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(UserRecord {
                user: row_to_user(&row)?,
                password_hash: row.try_get("password_hash")?,
            })),
        }
    }

    /// Fetch a user by id.
    pub async fn find_by_id(&self, user_id: &str) -> StorageResult<Option<User>> {
        let row = sqlx::query::<Db>(
            "SELECT user_id, username, role, enabled, display_name, avatar_url,
                    preferences, provider_filter, player_filter, created_at
             FROM users WHERE user_id = ?1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_user(&row)?)),
        }
    }

    /// Update the user's profile (display name / avatar).
    pub async fn update_profile(
        &self,
        user_id: &str,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
    ) -> StorageResult<Option<User>> {
        // We perform the update with whichever fields are non-null.
        // The simplest portable trick: update both columns, leaving
        // them unchanged when the caller passed `None`.
        let existing = self
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| StorageError::NotFound(user_id.into()))?;
        let new_display = display_name.map(str::to_string).or(existing.display_name);
        let new_avatar = avatar_url.map(str::to_string).or(existing.avatar_url);
        sqlx::query::<Db>("UPDATE users SET display_name = ?1, avatar_url = ?2 WHERE user_id = ?3")
            .bind(&new_display)
            .bind(&new_avatar)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        self.find_by_id(user_id).await
    }

    /// Insert or replace an auth token.
    pub async fn upsert_token(&self, token: &AuthToken) -> StorageResult<()> {
        let created_at = token.created_at.to_rfc3339();
        let expires_at = token.expires_at.map(|d| d.to_rfc3339());
        sqlx::query::<Db>(
            "INSERT INTO auth_tokens
                (token_id, user_id, token_hash, name, created_at, expires_at, last_used_at, is_long_lived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(token_id) DO UPDATE SET
                name=excluded.name,
                expires_at=excluded.expires_at,
                last_used_at=excluded.last_used_at,
                is_long_lived=excluded.is_long_lived",
        )
        .bind(&token.token_id)
        .bind(&token.user_id)
        .bind(&token.token_hash)
        .bind(&token.name)
        .bind(&created_at)
        .bind(&expires_at)
        .bind(token.last_used_at.map(|d| d.to_rfc3339()))
        .bind(token.is_long_lived)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look up an `AuthToken` by its plaintext-derived hash.
    pub async fn find_token_by_hash(&self, token_hash: &str) -> StorageResult<Option<AuthToken>> {
        let row = sqlx::query::<Db>(
            "SELECT token_id, user_id, token_hash, name, created_at, expires_at, last_used_at, is_long_lived
             FROM auth_tokens WHERE token_hash = ?1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_token(&row)?)),
        }
    }

    /// Revoke (delete) a token by its hash.
    pub async fn revoke_token(&self, token_hash: &str) -> StorageResult<bool> {
        let result = sqlx::query::<Db>("DELETE FROM auth_tokens WHERE token_hash = ?1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Count tokens for a user.
    pub async fn token_count_for_user(&self, user_id: &str) -> StorageResult<i64> {
        let row = sqlx::query::<Db>("SELECT COUNT(*) AS n FROM auth_tokens WHERE user_id = ?1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n").unwrap_or(0))
    }

    /// Count all tokens globally.
    pub async fn total_token_count(&self) -> StorageResult<i64> {
        let row = sqlx::query::<Db>("SELECT COUNT(*) AS n FROM auth_tokens")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n").unwrap_or(0))
    }

    /// Count users globally.
    pub async fn total_user_count(&self) -> StorageResult<i64> {
        let row = sqlx::query::<Db>("SELECT COUNT(*) AS n FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n").unwrap_or(0))
    }
}

fn row_to_user(row: &sqlx::any::AnyRow) -> StorageResult<User> {
    let role_str: String = row.try_get("role")?;
    let role: UserRole = match role_str.as_str() {
        "admin" => UserRole::Admin,
        "user" => UserRole::User,
        "guest" => UserRole::Guest,
        other => {
            return Err(StorageError::Backend(format!("unknown role: {other}")));
        }
    };
    let enabled: i64 = row.try_get("enabled").unwrap_or(1);
    let preferences: String = row.try_get("preferences").unwrap_or_else(|_| "null".into());
    let provider_filter: String = row
        .try_get("provider_filter")
        .unwrap_or_else(|_| "[]".into());
    let player_filter: String = row.try_get("player_filter").unwrap_or_else(|_| "[]".into());
    let created_at_str: String = row.try_get("created_at")?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    Ok(User {
        user_id: row.try_get("user_id")?,
        username: row.try_get("username")?,
        role,
        enabled: enabled != 0,
        created_at,
        display_name: row.try_get("display_name").ok(),
        avatar_url: row.try_get("avatar_url").ok(),
        preferences: serde_json::from_str(&preferences).unwrap_or(serde_json::Value::Null),
        provider_filter: serde_json::from_str(&provider_filter).unwrap_or_default(),
        player_filter: serde_json::from_str(&player_filter).unwrap_or_default(),
    })
}

fn row_to_token(row: &sqlx::any::AnyRow) -> StorageResult<AuthToken> {
    let created_at_str: String = row.try_get("created_at")?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let expires_at = row
        .try_get::<Option<String>, _>("expires_at")
        .ok()
        .flatten()
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));
    let last_used_at = row
        .try_get::<Option<String>, _>("last_used_at")
        .ok()
        .flatten()
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));
    let is_long_lived: i64 = row.try_get("is_long_lived").unwrap_or(1);
    Ok(AuthToken {
        token_id: row.try_get("token_id")?,
        user_id: row.try_get("user_id")?,
        token_hash: row.try_get("token_hash")?,
        name: row.try_get("name")?,
        created_at,
        expires_at,
        last_used_at,
        is_long_lived: is_long_lived != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{Database, DatabaseConfig};

    async fn repo() -> AuthRepository {
        let db = Database::connect(DatabaseConfig::in_memory_sqlite())
            .await
            .unwrap();
        AuthRepository::new(db.pool().clone())
    }

    fn user(name: &str, role: UserRole) -> User {
        User {
            username: name.into(),
            role,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn empty_users_returns_false() {
        let r = repo().await;
        assert!(!r.has_users().await.unwrap());
    }

    #[tokio::test]
    async fn upsert_and_find_user() {
        let r = repo().await;
        let u = user("alice", UserRole::Admin);
        r.upsert_user(&u, "hash1").await.unwrap();
        let rec = r.find_by_username("alice").await.unwrap().unwrap();
        assert_eq!(rec.user.user_id, u.user_id);
        assert_eq!(rec.password_hash, "hash1");
        assert!(r.has_users().await.unwrap());
    }

    #[tokio::test]
    async fn update_profile_changes_fields() {
        let r = repo().await;
        let u = user("bob", UserRole::User);
        r.upsert_user(&u, "h").await.unwrap();
        r.update_profile(&u.user_id, Some("Bob"), Some("https://x/y"))
            .await
            .unwrap();
        let rec = r.find_by_username("bob").await.unwrap().unwrap();
        assert_eq!(rec.user.display_name.as_deref(), Some("Bob"));
        assert_eq!(rec.user.avatar_url.as_deref(), Some("https://x/y"));
    }

    #[tokio::test]
    async fn tokens_round_trip() {
        let r = repo().await;
        let u = user("c", UserRole::User);
        r.upsert_user(&u, "h").await.unwrap();
        let token = AuthToken {
            user_id: u.user_id.clone(),
            token_hash: "hash-abc".into(),
            name: "test".into(),
            ..Default::default()
        };
        r.upsert_token(&token).await.unwrap();
        let found = r.find_token_by_hash("hash-abc").await.unwrap().unwrap();
        assert_eq!(found.token_id, token.token_id);
        assert!(r.revoke_token("hash-abc").await.unwrap());
        assert!(r.find_token_by_hash("hash-abc").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn token_counts() {
        let r = repo().await;
        let u = user("d", UserRole::User);
        r.upsert_user(&u, "h").await.unwrap();
        for i in 0..3 {
            let t = AuthToken {
                user_id: u.user_id.clone(),
                token_hash: format!("h-{i}"),
                ..Default::default()
            };
            r.upsert_token(&t).await.unwrap();
        }
        assert_eq!(r.token_count_for_user(&u.user_id).await.unwrap(), 3);
        assert_eq!(r.total_token_count().await.unwrap(), 3);
        assert_eq!(r.total_user_count().await.unwrap(), 1);
    }
}
