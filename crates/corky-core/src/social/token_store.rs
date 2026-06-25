//! Token store for social media OAuth tokens.
//!
//! Stores tokens keyed by URN in ~/.config/corky/tokens.json with 0600 permissions.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::path::PathBuf;

use crate::app_config;
use crate::file_store;

/// A stored OAuth token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredToken {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub scopes: Vec<String>,
    pub platform: String,
}

/// Grace window: tokens expiring within this many seconds are considered expired.
const GRACE_SECONDS: i64 = 300; // 5 minutes

impl StoredToken {
    /// Check if the token is still valid (not expired, accounting for grace window).
    pub fn is_valid(&self) -> bool {
        let now = Utc::now();
        let grace = chrono::Duration::seconds(GRACE_SECONDS);
        self.expires_at > now + grace
    }
}

/// URN-keyed token store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TokenStore {
    pub tokens: HashMap<String, StoredToken>,
    #[serde(skip)]
    loaded_tokens: HashMap<String, StoredToken>,
    #[serde(skip)]
    dirty_tokens: HashSet<String>,
    #[serde(skip)]
    deleted_tokens: HashSet<String>,
}

/// Return the path to tokens.json.
pub fn tokens_path() -> PathBuf {
    app_config::app_config_dir().join("tokens.json")
}

impl TokenStore {
    /// Load the token store from disk. Returns empty store if file doesn't exist.
    pub fn load() -> Result<Self> {
        Self::load_from(&tokens_path())
    }

    /// Load from a specific path.
    pub fn load_from(path: &Path) -> Result<Self> {
        let mut store: Self = file_store::load_json_or_default(path)
            .with_context(|| format!("Failed to load tokens from {}", path.display()))?;
        store.loaded_tokens = store.tokens.clone();
        Ok(store)
    }

    /// Save the token store to disk with 0600 permissions.
    pub fn save(&self) -> Result<()> {
        self.save_to(&tokens_path())
    }

    /// Save to a specific path.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        file_store::save_json_with_lock::<TokenStore, _>(path, Some(0o600), |mut current| {
            for urn in self.loaded_tokens.keys() {
                if !self.tokens.contains_key(urn) || self.deleted_tokens.contains(urn) {
                    current.tokens.remove(urn);
                }
            }

            for (urn, token) in &self.tokens {
                match self.loaded_tokens.get(urn) {
                    Some(loaded) if loaded == token && !self.dirty_tokens.contains(urn) => {}
                    _ => {
                        current.tokens.insert(urn.clone(), token.clone());
                    }
                }
            }
            Ok(current)
        })
    }

    /// Get a valid (non-expired) token for a URN.
    pub fn get_valid(&self, urn: &str) -> Option<&StoredToken> {
        self.tokens.get(urn).filter(|t| t.is_valid())
    }

    /// Insert or update a token for a URN.
    pub fn upsert(&mut self, urn: String, token: StoredToken) {
        self.deleted_tokens.remove(&urn);
        self.dirty_tokens.insert(urn.clone());
        self.tokens.insert(urn, token);
    }

    /// Remove a token by URN.
    pub fn remove(&mut self, urn: &str) -> Option<StoredToken> {
        self.dirty_tokens.remove(urn);
        self.deleted_tokens.insert(urn.to_string());
        self.tokens.remove(urn)
    }

    /// Remove a token by URN and persist the deletion under the store lock.
    pub fn remove_persisted(&mut self, urn: &str) -> Result<bool> {
        self.remove_persisted_from(&tokens_path(), urn)
    }

    /// Remove a token from a specific path and persist the deletion atomically.
    pub fn remove_persisted_from(&mut self, path: &Path, urn: &str) -> Result<bool> {
        let removed_in_memory = self.remove(urn).is_some();
        let mut removed_on_disk = false;
        file_store::save_json_with_lock::<TokenStore, _>(path, Some(0o600), |mut current| {
            removed_on_disk = current.tokens.remove(urn).is_some();
            Ok(current)
        })?;
        Ok(removed_in_memory || removed_on_disk)
    }

    /// Remove every persisted token whose `access_token` matches, returning true
    /// if at least one was removed (#ckytok401).
    ///
    /// Used to invalidate a token the server rejected with `401` when the caller
    /// holds only the access-token string, not the store key — so the next API
    /// call re-authenticates instead of replaying the revoked token.
    pub fn remove_by_access_token(&mut self, access_token: &str) -> Result<bool> {
        self.remove_by_access_token_from(&tokens_path(), access_token)
    }

    /// [`remove_by_access_token`](Self::remove_by_access_token) against a specific path.
    pub fn remove_by_access_token_from(
        &mut self,
        path: &Path,
        access_token: &str,
    ) -> Result<bool> {
        let in_memory_keys: Vec<String> = self
            .tokens
            .iter()
            .filter(|(_, t)| t.access_token == access_token)
            .map(|(k, _)| k.clone())
            .collect();
        let removed_in_memory = in_memory_keys.iter().any(|k| self.remove(k).is_some());

        let mut removed_on_disk = false;
        file_store::save_json_with_lock::<TokenStore, _>(path, Some(0o600), |mut current| {
            let disk_keys: Vec<String> = current
                .tokens
                .iter()
                .filter(|(_, t)| t.access_token == access_token)
                .map(|(k, _)| k.clone())
                .collect();
            for k in &disk_keys {
                if current.tokens.remove(k).is_some() {
                    removed_on_disk = true;
                }
            }
            Ok(current)
        })?;
        Ok(removed_in_memory || removed_on_disk)
    }
}

/// Invalidate the cached token the server rejected with `401` so the next API
/// call re-authenticates (#ckytok401). No-op (returns `Ok(false)`) when the
/// token string is empty or no cached token matches.
pub fn clear_access_token(access_token: &str) -> Result<bool> {
    if access_token.is_empty() {
        return Ok(false);
    }
    let mut store = TokenStore::load()?;
    store.remove_by_access_token(access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(value: &str) -> StoredToken {
        StoredToken {
            access_token: value.to_string(),
            refresh_token: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            scopes: vec!["scope".to_string()],
            platform: "test".to_string(),
        }
    }

    #[test]
    fn remove_by_access_token_clears_matching_entry() {
        // #ckytok401: a 401 invalidates the cached token by its access-token
        // value, leaving other accounts untouched.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        let mut store = TokenStore::default();
        store.upsert("gmail:a".to_string(), token("revoked-xyz"));
        store.upsert("gmail:b".to_string(), token("still-good"));
        store.save_to(&path).unwrap();

        let removed = store.remove_by_access_token_from(&path, "revoked-xyz").unwrap();
        assert!(removed, "should report a removal");

        let reloaded = TokenStore::load_from(&path).unwrap();
        assert!(!reloaded.tokens.contains_key("gmail:a"), "revoked token gone");
        assert!(reloaded.tokens.contains_key("gmail:b"), "other token kept");

        // No match → no-op, returns false.
        let mut store2 = TokenStore::load_from(&path).unwrap();
        assert!(!store2.remove_by_access_token_from(&path, "nonexistent").unwrap());
    }

    #[test]
    fn save_preserves_unrelated_tokens_from_concurrent_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        let mut initial = TokenStore::default();
        initial.upsert("shared".to_string(), token("base"));
        initial.save_to(&path).unwrap();

        let mut writer_a = TokenStore::load_from(&path).unwrap();
        let mut writer_b = TokenStore::load_from(&path).unwrap();

        writer_a.upsert("gmail:a".to_string(), token("a"));
        writer_a.save_to(&path).unwrap();

        writer_b.upsert("gmail:b".to_string(), token("b"));
        writer_b.save_to(&path).unwrap();

        let merged = TokenStore::load_from(&path).unwrap();
        assert_eq!(merged.tokens["shared"].access_token, "base");
        assert_eq!(merged.tokens["gmail:a"].access_token, "a");
        assert_eq!(merged.tokens["gmail:b"].access_token, "b");
    }

    #[test]
    fn stale_save_does_not_restore_concurrently_deleted_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        let mut initial = TokenStore::default();
        initial.upsert("gmail:a:send".to_string(), token("a"));
        initial.save_to(&path).unwrap();

        let mut stale_writer = TokenStore::load_from(&path).unwrap();
        let mut remover = TokenStore::load_from(&path).unwrap();

        assert!(
            remover
                .remove_persisted_from(&path, "gmail:a:send")
                .unwrap()
        );

        stale_writer.upsert("gmail:b:send".to_string(), token("b"));
        stale_writer.save_to(&path).unwrap();

        let persisted = TokenStore::load_from(&path).unwrap();
        assert!(!persisted.tokens.contains_key("gmail:a:send"));
        assert_eq!(persisted.tokens["gmail:b:send"].access_token, "b");
    }

    #[test]
    fn explicit_upsert_can_readd_concurrently_deleted_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        let mut initial = TokenStore::default();
        initial.upsert("gmail:a:send".to_string(), token("old"));
        initial.save_to(&path).unwrap();

        let mut writer = TokenStore::load_from(&path).unwrap();
        let mut remover = TokenStore::load_from(&path).unwrap();

        remover
            .remove_persisted_from(&path, "gmail:a:send")
            .unwrap();

        writer.upsert("gmail:a:send".to_string(), token("new"));
        writer.save_to(&path).unwrap();

        let persisted = TokenStore::load_from(&path).unwrap();
        assert_eq!(persisted.tokens["gmail:a:send"].access_token, "new");
    }

    #[test]
    fn remove_persisted_deletes_only_requested_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        let mut initial = TokenStore::default();
        initial.upsert("gmail:a:send".to_string(), token("a"));
        initial.upsert("gmail:b:send".to_string(), token("b"));
        initial.save_to(&path).unwrap();

        let mut store = TokenStore::load_from(&path).unwrap();
        assert!(store.remove_persisted_from(&path, "gmail:a:send").unwrap());

        let persisted = TokenStore::load_from(&path).unwrap();
        assert!(!persisted.tokens.contains_key("gmail:a:send"));
        assert_eq!(persisted.tokens["gmail:b:send"].access_token, "b");
    }
}
