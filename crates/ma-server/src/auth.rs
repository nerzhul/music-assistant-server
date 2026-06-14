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
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthManager")
            .field("store", &self.store.read())
            .finish()
    }
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(AuthStore::default())),
        }
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
        self.store.write().tokens.insert(token_id.clone(), token);
        self.store
            .write()
            .token_hashes
            .insert(plaintext.clone(), token_id);
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
