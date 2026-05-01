//! Token store for social media OAuth tokens.
//!
//! Stores tokens keyed by URN in ~/.config/corky/tokens.json with 0600 permissions.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
        file_store::load_json_or_default(path)
            .with_context(|| format!("Failed to load tokens from {}", path.display()))
    }

    /// Save the token store to disk with 0600 permissions.
    pub fn save(&self) -> Result<()> {
        self.save_to(&tokens_path())
    }

    /// Save to a specific path.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        file_store::save_json_with_lock::<TokenStore, _>(path, Some(0o600), |mut current| {
            current.tokens.extend(self.tokens.clone());
            Ok(current)
        })
    }

    /// Get a valid (non-expired) token for a URN.
    pub fn get_valid(&self, urn: &str) -> Option<&StoredToken> {
        self.tokens.get(urn).filter(|t| t.is_valid())
    }

    /// Insert or update a token for a URN.
    pub fn upsert(&mut self, urn: String, token: StoredToken) {
        self.tokens.insert(urn, token);
    }

    /// Remove a token by URN.
    pub fn remove(&mut self, urn: &str) -> Option<StoredToken> {
        self.tokens.remove(urn)
    }

    /// Remove a token by URN and persist the deletion under the store lock.
    pub fn remove_persisted(&mut self, urn: &str) -> Result<bool> {
        self.remove_persisted_from(&tokens_path(), urn)
    }

    /// Remove a token from a specific path and persist the deletion atomically.
    pub fn remove_persisted_from(&mut self, path: &Path, urn: &str) -> Result<bool> {
        let removed_in_memory = self.tokens.remove(urn).is_some();
        let mut removed_on_disk = false;
        file_store::save_json_with_lock::<TokenStore, _>(path, Some(0o600), |mut current| {
            removed_on_disk = current.tokens.remove(urn).is_some();
            Ok(current)
        })?;
        Ok(removed_in_memory || removed_on_disk)
    }
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
