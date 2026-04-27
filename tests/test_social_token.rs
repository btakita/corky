//! Token store edge cases (T1–T9).

mod common;

use chrono::{Duration, Utc};
use corky::social::token_store::{StoredToken, TokenStore};
use tempfile::TempDir;

fn make_token(expires_in_secs: i64) -> StoredToken {
    StoredToken {
        access_token: "test-access-token".to_string(),
        refresh_token: Some("test-refresh-token".to_string()),
        expires_at: Utc::now() + Duration::seconds(expires_in_secs),
        scopes: vec!["openid".to_string(), "profile".to_string()],
        platform: "linkedin".to_string(),
    }
}

// T1: Missing tokens.json → returns empty store
#[test]
fn t1_missing_tokens_json() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("tokens.json");
    let store = TokenStore::load_from(&path).unwrap();
    assert!(store.tokens.is_empty());
}

// T2: Save/load round-trip
#[test]
fn t2_save_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("tokens.json");

    let mut store = TokenStore::default();
    store.upsert("urn:li:person:123".to_string(), make_token(3600));
    store.save_to(&path).unwrap();

    let loaded = TokenStore::load_from(&path).unwrap();
    assert_eq!(loaded.tokens.len(), 1);
    let token = loaded.tokens.get("urn:li:person:123").unwrap();
    assert_eq!(token.access_token, "test-access-token");
    assert_eq!(token.refresh_token, Some("test-refresh-token".to_string()));
    assert_eq!(token.scopes, vec!["openid", "profile"]);
    assert_eq!(token.platform, "linkedin");
}

// T3: Get valid token
#[test]
fn t3_get_valid_token() {
    let mut store = TokenStore::default();
    store.upsert("urn:li:person:123".to_string(), make_token(3600)); // 1 hour

    let token = store.get_valid("urn:li:person:123");
    assert!(token.is_some());
    assert_eq!(token.unwrap().access_token, "test-access-token");
}

// T4: Get expired token → None
#[test]
fn t4_get_expired_token() {
    let mut store = TokenStore::default();
    store.upsert("urn:li:person:123".to_string(), make_token(-1)); // already expired

    let token = store.get_valid("urn:li:person:123");
    assert!(token.is_none());
}

// T5: Token in grace window (<5 min remaining) → None
#[test]
fn t5_token_in_grace_window() {
    let mut store = TokenStore::default();
    // Expires in 4 minutes — within 5-minute grace window
    store.upsert("urn:li:person:123".to_string(), make_token(240));

    let token = store.get_valid("urn:li:person:123");
    assert!(token.is_none(), "Token within grace window should be None");
}

// T6: Multiple tokens for different URNs
#[test]
fn t6_multiple_urns() {
    let mut store = TokenStore::default();
    let mut token1 = make_token(3600);
    token1.access_token = "token-for-alice".to_string();
    let mut token2 = make_token(3600);
    token2.access_token = "token-for-bob".to_string();

    store.upsert("urn:li:person:alice".to_string(), token1);
    store.upsert("urn:li:person:bob".to_string(), token2);

    assert_eq!(store.tokens.len(), 2);
    assert_eq!(
        store.get_valid("urn:li:person:alice").unwrap().access_token,
        "token-for-alice"
    );
    assert_eq!(
        store.get_valid("urn:li:person:bob").unwrap().access_token,
        "token-for-bob"
    );
}

// T7: Upsert overwrites existing
#[test]
fn t7_upsert_overwrites() {
    let mut store = TokenStore::default();
    let mut token1 = make_token(3600);
    token1.access_token = "old-token".to_string();
    store.upsert("urn:li:person:123".to_string(), token1);

    let mut token2 = make_token(7200);
    token2.access_token = "new-token".to_string();
    store.upsert("urn:li:person:123".to_string(), token2);

    assert_eq!(store.tokens.len(), 1);
    assert_eq!(
        store.get_valid("urn:li:person:123").unwrap().access_token,
        "new-token"
    );
}

// T8: File permissions 0600 on save
#[cfg(unix)]
#[test]
fn t8_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("tokens.json");

    let store = TokenStore::default();
    store.save_to(&path).unwrap();

    let metadata = std::fs::metadata(&path).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "Token file should have 0600 permissions, got {:o}",
        mode
    );
}

// T9: Malformed tokens.json
#[test]
fn t9_malformed_json() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("tokens.json");
    std::fs::write(&path, "not valid json {{{").unwrap();

    let result = TokenStore::load_from(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(!err.is_empty(), "Error should have parse details");
}

// Additional: remove token
#[test]
fn remove_token() {
    let mut store = TokenStore::default();
    store.upsert("urn:li:person:123".to_string(), make_token(3600));

    let removed = store.remove("urn:li:person:123");
    assert!(removed.is_some());
    assert!(store.tokens.is_empty());
}
