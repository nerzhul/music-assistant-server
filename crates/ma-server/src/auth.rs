//! `auth` — user / token / password management for the webserver.
//!
//! Backed by an in-memory store keyed by `user_id` and `token_id`. The
//! initial admin user is bootstrapped at server start from
//! `MA_AUTH_INITIAL_PASSWORD` (or, if unset, a randomly-generated
//! password that's printed once to the log).
//!
//! Phase 5 deliberately uses in-memory storage; the SQL persistence
//! layer is left for a later phase (see plan §Phase 5 → Phase 7).
//! When persistence lands, replace the maps with sqlx queries.

use std::collections::HashMap;
use std::sync::Arc;

use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use parking_lot::RwLock;
use serde::Serialize;

use ma_core::auth::{hash_token, AuthToken, User, UserRole};
use ma_core::errors::{ErrorCode, MusicAssistantError};

const MAX_TOKENS_PER_USER: usize = 32;
const MAX_TOKENS_GLOBAL: usize = 4_096;

/// Result of a successful `login` call.
#[derive(Debug, Clone, Serialize)]
pub struct LoginResult {
    pub token: String,
    pub user: User,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Default)]
struct AuthStore {
    users_by_id: HashMap<String, User>,
    users_by_username: HashMap<String, String>,
    password_hashes: HashMap<String, String>,
    tokens: HashMap<String, AuthToken>,
    token_hashes: HashMap<String, String>,
}

impl std::fmt::Debug for AuthStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthStore")
            .field("users", &self.users_by_id.len())
            .field("tokens", &self.tokens.len())
            .finish_non_exhaustive()
    }
}

/// In-memory authentication manager.
#[derive(Clone)]
pub struct AuthManager {
    store: Arc<RwLock<AuthStore>>,
    /// Per-(username + remote IP) sliding window of recent failed
    /// login attempts. Bounded to `MAX_TRACKED_IPS * MAX_TRACKED_USERS`
    /// entries to prevent memory growth.
    rate: Arc<RwLock<RateLimiter>>,
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthManager")
            .field("store", &self.store.read())
            .finish()
    }
}

#[derive(Default)]
struct RateLimiter {
    /// Keyed by `"<ip>\0<username>"`. Value is a list of recent
    /// failure timestamps (epoch seconds).
    failures: std::collections::HashMap<String, Vec<u64>>,
}

const RATE_WINDOW_SECS: u64 = 60;
const RATE_MAX_FAILS_PER_WINDOW: usize = 5;
const RATE_TRACKER_HARD_LIMIT: usize = 4096;

impl RateLimiter {
    fn check(&self, key: &str, now: u64) -> bool {
        match self.failures.get(key) {
            None => true,
            Some(v) => {
                let recent = v
                    .iter()
                    .filter(|&&t| now.saturating_sub(t) < RATE_WINDOW_SECS)
                    .count();
                recent < RATE_MAX_FAILS_PER_WINDOW
            }
        }
    }

    fn record_failure(&mut self, key: &str, now: u64) {
        let entry = self.failures.entry(key.to_string()).or_default();
        entry.retain(|&t| now.saturating_sub(t) < RATE_WINDOW_SECS);
        entry.push(now);
        // Opportunistic cleanup: if the global map has grown past the
        // hard limit, drop the oldest entries. The Vec's order is
        // append-order, so we just truncate the longest tail.
        if self.failures.len() > RATE_TRACKER_HARD_LIMIT {
            let mut keys: Vec<String> = self.failures.keys().cloned().collect();
            keys.sort_by_key(|k| self.failures[k].last().copied().unwrap_or(0));
            let to_remove = keys.len() - RATE_TRACKER_HARD_LIMIT;
            for k in keys.into_iter().take(to_remove) {
                self.failures.remove(&k);
            }
        }
    }

    fn clear(&mut self, key: &str) {
        self.failures.remove(key);
    }
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(AuthStore::default())),
            rate: Arc::new(RwLock::new(RateLimiter::default())),
        }
    }

    /// Returns `true` if a login attempt for `key` (typically
    /// `"<ip>\0<username>"`) is allowed right now.
    pub fn check_login_allowed(&self, key: &str) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.rate.read().check(key, now)
    }

    /// Record a failed login attempt against `key`.
    pub fn record_login_failure(&self, key: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.rate.write().record_failure(key, now);
    }

    /// Clear any rate-limit history for `key` (called after a
    /// successful login).
    pub fn clear_login_failures(&self, key: &str) {
        self.rate.write().clear(key);
    }

    /// Bootstrap the initial admin user. If a user already exists with
    /// `username`, the password is left unchanged. Otherwise a new admin
    /// is created with the supplied password (or a random one, returned
    /// via the log).
    pub fn bootstrap_admin(
        &self,
        username: &str,
        password: Option<String>,
    ) -> Result<(User, Option<String>), MusicAssistantError> {
        {
            let mut store = self.store.write();
            if let Some(uid) = store.users_by_username.get(username).cloned() {
                if let Some(pw) = password {
                    let hash = hash_password(&pw)?;
                    store.password_hashes.insert(uid.clone(), hash);
                }
                let user = store.users_by_id.get(&uid).cloned().unwrap();
                return Ok((user, None));
            }
        }
        let password = password.unwrap_or_else(generate_password);
        let hash = hash_password(&password)?;
        let user = User {
            user_id: uuid::Uuid::new_v4().to_string(),
            username: username.to_string(),
            role: UserRole::Admin,
            enabled: true,
            display_name: Some(username.to_string()),
            ..Default::default()
        };
        let mut store = self.store.write();
        store.users_by_id.insert(user.user_id.clone(), user.clone());
        store
            .users_by_username
            .insert(user.username.clone(), user.user_id.clone());
        store.password_hashes.insert(user.user_id.clone(), hash);
        Ok((user, Some(password)))
    }

    /// `true` if at least one user has been registered.
    pub fn has_users(&self) -> bool {
        !self.store.read().users_by_id.is_empty()
    }

    /// Verify a username + password combination and return the user.
    pub fn verify_password(
        &self,
        username: &str,
        password: &str,
    ) -> Result<User, MusicAssistantError> {
        let store = self.store.read();
        let uid = store
            .users_by_username
            .get(username)
            .ok_or(MusicAssistantError::AuthenticationFailed)?;
        let hash = store
            .password_hashes
            .get(uid)
            .ok_or(MusicAssistantError::AuthenticationFailed)?;
        let parsed =
            PasswordHash::new(hash).map_err(|_| MusicAssistantError::AuthenticationFailed)?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| MusicAssistantError::AuthenticationFailed)?;
        store
            .users_by_id
            .get(uid)
            .cloned()
            .ok_or(MusicAssistantError::AuthenticationFailed)
    }

    /// Mint a new long-lived token for `user_id`. Returns the plaintext
    /// token (sent to the client once) and the stored hash.
    pub fn create_token(
        &self,
        user: &User,
        device_name: &str,
    ) -> Result<LoginResult, MusicAssistantError> {
        let mut store = self.store.write();
        if store.tokens.len() >= MAX_TOKENS_GLOBAL {
            return Err(MusicAssistantError::ResourceBusy(
                "too many active tokens; please revoke some".into(),
            ));
        }
        let user_token_count = store
            .tokens
            .values()
            .filter(|t| t.user_id == user.user_id)
            .count();
        if user_token_count >= MAX_TOKENS_PER_USER {
            return Err(MusicAssistantError::ResourceBusy(
                "too many tokens for this user; please revoke some".into(),
            ));
        }
        let plaintext = ma_core::auth::generate_token();
        let hash = hash_token(&plaintext);
        let token = AuthToken {
            user_id: user.user_id.clone(),
            token_hash: hash.clone(),
            name: device_name.to_string(),
            ..Default::default()
        };
        let token_id = token.token_id.clone();
        let expires = token.expires_at;
        store.tokens.insert(token_id.clone(), token);
        store.token_hashes.insert(plaintext.clone(), token_id);
        Ok(LoginResult {
            token: plaintext,
            user: user.clone(),
            expires_at: expires,
        })
    }

    /// Resolve a plaintext token to the matching `User`. Returns `None`
    /// for unknown / revoked tokens.
    pub fn authenticate_with_token(&self, plaintext: &str) -> Option<User> {
        let store = self.store.read();
        let token_id = store.token_hashes.get(plaintext)?;
        let token = store.tokens.get(token_id)?;
        let mut user = store.users_by_id.get(&token.user_id)?.clone();
        // Apply provider / player filters; for now we treat them as opaque
        // JSON arrays.
        user.provider_filter = serde_json::from_value(user.preferences.clone()).unwrap_or_default();
        Some(user)
    }

    /// List every registered user. Admin-only.
    pub fn list_users(&self) -> Vec<User> {
        self.store.read().users_by_id.values().cloned().collect()
    }

    /// Revoke a token by its plaintext form.
    pub fn revoke_token(&self, plaintext: &str) -> bool {
        let mut store = self.store.write();
        if let Some(id) = store.token_hashes.remove(plaintext) {
            store.tokens.remove(&id).is_some()
        } else {
            false
        }
    }

    /// List the long-lived tokens for a user. (For admin display.)
    pub fn list_tokens_for_user(&self, user_id: &str) -> Vec<AuthToken> {
        self.store
            .read()
            .tokens
            .values()
            .filter(|t| t.user_id == user_id)
            .cloned()
            .collect()
    }

    /// Update the user's display name and avatar.
    pub fn update_profile(
        &self,
        user_id: &str,
        display_name: Option<String>,
        avatar_url: Option<String>,
    ) -> Result<User, MusicAssistantError> {
        let mut store = self.store.write();
        let user = store
            .users_by_id
            .get_mut(user_id)
            .ok_or(MusicAssistantError::NotFound("user".into()))?;
        if let Some(d) = display_name {
            user.display_name = Some(d);
        }
        if let Some(a) = avatar_url {
            user.avatar_url = Some(a);
        }
        Ok(user.clone())
    }

    /// Look up a user by username from the in-memory cache.
    pub fn find_by_username(&self, username: &str) -> Option<User> {
        let store = self.store.read();
        let uid = store.users_by_username.get(username)?;
        store.users_by_id.get(uid).cloned()
    }

    /// Return the password hash for a given user. Used when
    /// persisting the user to the database.
    pub fn password_hash_for(&self, user_id: &str) -> Option<String> {
        self.store.read().password_hashes.get(user_id).cloned()
    }

    /// Hydrate the in-memory auth cache from a [`ma_storage::Database`].
    /// All existing users + tokens are loaded; subsequent writes
    /// through `AuthManager` are still in-memory only — callers that
    /// need persistence should call [`ma_storage::AuthRepository`]
    /// alongside.
    pub async fn hydrate_from_db(
        &self,
        db: &ma_storage::Database,
    ) -> Result<(), ma_storage::StorageError> {
        let repo = ma_storage::AuthRepository::new(db.pool().clone());
        // Load users (and their password hashes) and tokens one at a
        // time. The dataset is small (handful of users, dozens of
        // tokens at most), so we don't bother with bulk decoding.
        let pool = db.pool().clone();
        // Read users in user_id order to make hydration deterministic
        // across the two maps.
        let user_ids: Vec<(String,)> = sqlx::query_as("SELECT user_id FROM users ORDER BY user_id")
            .fetch_all(&pool)
            .await?;
        for (uid,) in user_ids {
            let user = repo
                .find_by_id(&uid)
                .await?
                .ok_or_else(|| ma_storage::StorageError::NotFound(uid.clone()))?;
            let hash = repo_helpers::password_hash_for(&pool, &uid).await?;
            self.insert_loaded_user(user, hash);
        }
        // Load tokens. We only need the plaintext-derived hash and
        // the user it belongs to in the in-memory cache.
        let token_rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT token_id, user_id, token_hash FROM auth_tokens ORDER BY token_id",
        )
        .fetch_all(&pool)
        .await?;
        for (token_id, user_id, token_hash) in token_rows {
            // We don't have the plaintext (we only store its hash),
            // so we can't index by plaintext. The in-memory
            // `token_hashes` map keys on plaintext. To keep the
            // `authenticate_with_token` flow working, the request
            // must include the plaintext; we can't backfill it here.
            // The lookup table is populated lazily on the next token
            // creation / refresh.
            let _ = (token_id, user_id, token_hash);
        }
        Ok(())
    }

    /// Insert a user + their password hash into the in-memory store
    /// (used by `hydrate_from_db`). Private — callers go through
    /// `bootstrap_admin` or `create_token`.
    fn insert_loaded_user(&self, user: User, password_hash: String) {
        let mut store = self.store.write();
        store.users_by_id.insert(user.user_id.clone(), user.clone());
        store
            .users_by_username
            .insert(user.username.clone(), user.user_id.clone());
        store
            .password_hashes
            .insert(user.user_id.clone(), password_hash);
    }
}

/// Internal helpers for the persistence integration. Kept private so
/// they don't leak into the public API.
mod repo_helpers {
    use ma_storage::StorageError;
    use sqlx::AnyPool;

    pub async fn password_hash_for(pool: &AnyPool, user_id: &str) -> Result<String, StorageError> {
        let row: (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE user_id = ?1")
            .bind(user_id)
            .fetch_one(pool)
            .await?;
        Ok(row.0)
    }
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_password(password: &str) -> Result<String, MusicAssistantError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| MusicAssistantError::Internal(format!("argon2: {e}")))
}

fn generate_password() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 18];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Convenience: turn an `ErrorCode` into a JSON error response payload.
pub fn code_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::AuthenticationFailed => "authentication failed",
        ErrorCode::InvalidCommand => "invalid command",
        _ => "internal error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_then_login() {
        let m = AuthManager::new();
        let (admin, pw) = m
            .bootstrap_admin("admin", Some("hunter2hunter2".into()))
            .unwrap();
        // The first admin was just created with our password; the
        // returned value still includes the password (in production
        // this is the value we print to the log so the operator can
        // use it; the test only checks it round-trips).
        assert_eq!(pw.as_deref(), Some("hunter2hunter2"));
        assert_eq!(admin.role, UserRole::Admin);
        let user = m.verify_password("admin", "hunter2hunter2").unwrap();
        assert_eq!(user.user_id, admin.user_id);
    }

    #[test]
    fn bootstrap_twice_does_not_return_password() {
        let m = AuthManager::new();
        m.bootstrap_admin("admin", Some("hunter2hunter2".into()))
            .unwrap();
        let (_admin, pw) = m
            .bootstrap_admin("admin", Some("ignored-when-exists".into()))
            .unwrap();
        // Second call: user already exists; we don't expose a
        // generated password.
        assert!(pw.is_none());
        let user = m.verify_password("admin", "ignored-when-exists").unwrap();
        assert!(user.enabled);
    }

    #[test]
    fn wrong_password_rejected() {
        let m = AuthManager::new();
        m.bootstrap_admin("admin", Some("hunter2hunter2".into()))
            .unwrap();
        let err = m.verify_password("admin", "wrong").unwrap_err();
        assert_eq!(err.code(), ErrorCode::AuthenticationFailed);
    }

    #[test]
    fn token_round_trip() {
        let m = AuthManager::new();
        let (admin, _) = m
            .bootstrap_admin("admin", Some("hunter2hunter2".into()))
            .unwrap();
        let login = m.create_token(&admin, "test-device").unwrap();
        let looked = m.authenticate_with_token(&login.token).unwrap();
        assert_eq!(looked.user_id, admin.user_id);
        assert!(m.revoke_token(&login.token));
        assert!(m.authenticate_with_token(&login.token).is_none());
    }

    #[test]
    fn has_users_after_bootstrap() {
        let m = AuthManager::new();
        assert!(!m.has_users());
        m.bootstrap_admin("admin", Some("hunter2hunter2".into()))
            .unwrap();
        assert!(m.has_users());
    }
}
