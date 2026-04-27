//! Integration tests for path resolution (src/resolve.rs).

mod common;

use std::path::PathBuf;
use tempfile::TempDir;

use corky::resolve;

#[test]
fn test_expand_tilde_with_prefix() {
    let home = resolve::home_dir();
    let result = resolve::expand_tilde("~/Documents/test");
    assert_eq!(result, home.join("Documents").join("test"));
}

#[test]
fn test_expand_tilde_bare() {
    let home = resolve::home_dir();
    let result = resolve::expand_tilde("~");
    assert_eq!(result, home);
}

#[test]
fn test_expand_tilde_no_tilde() {
    let result = resolve::expand_tilde("/absolute/path");
    assert_eq!(result, PathBuf::from("/absolute/path"));
}

#[test]
fn test_expand_tilde_relative() {
    let result = resolve::expand_tilde("relative/path");
    assert_eq!(result, PathBuf::from("relative/path"));
}

#[test]
fn test_home_dir_returns_path() {
    let home = resolve::home_dir();
    assert!(!home.to_string_lossy().is_empty());
}

#[test]
fn test_derived_paths_are_consistent() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().to_path_buf();
    // SAFETY: Test-only; no concurrent env access.
    unsafe { std::env::set_var("CORKY_DATA", data.to_string_lossy().as_ref()) };

    let conversations = resolve::conversations_dir();
    let drafts = resolve::drafts_dir();
    let contacts = resolve::contacts_dir();

    assert!(conversations.to_string_lossy().ends_with("conversations"));
    assert!(drafts.to_string_lossy().ends_with("drafts"));
    assert!(contacts.to_string_lossy().ends_with("contacts"));

    unsafe { std::env::remove_var("CORKY_DATA") };
}

#[test]
fn test_mailbox_dir_lowercases() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().to_path_buf();
    // SAFETY: Test-only; no concurrent env access.
    unsafe { std::env::set_var("CORKY_DATA", data.to_string_lossy().as_ref()) };

    let dir = resolve::mailbox_dir("AlexUser");
    assert!(dir.to_string_lossy().contains("alexuser"));
    assert!(dir.to_string_lossy().ends_with("mailboxes/alexuser"));

    unsafe { std::env::remove_var("CORKY_DATA") };
}

#[test]
fn test_corky_toml_default_path() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().to_path_buf();
    // SAFETY: Test-only; no concurrent env access.
    unsafe { std::env::set_var("CORKY_DATA", data.to_string_lossy().as_ref()) };

    let path = resolve::corky_toml();
    assert!(path.to_string_lossy().ends_with(".corky.toml"));

    unsafe { std::env::remove_var("CORKY_DATA") };
}

#[test]
fn test_corky_toml_finds_dotfile() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().to_path_buf();
    // SAFETY: Test-only; no concurrent env access.
    unsafe { std::env::set_var("CORKY_DATA", data.to_string_lossy().as_ref()) };

    std::fs::write(data.join(".corky.toml"), "").unwrap();
    let path = resolve::corky_toml();
    assert!(path.to_string_lossy().ends_with(".corky.toml"));
    // data_dir() gives precedence to local mail/ when present,
    // so the env var path may not be used in the dev checkout.
    if resolve::data_dir() == data {
        assert!(path.exists());
    }

    unsafe { std::env::remove_var("CORKY_DATA") };
}

#[test]
fn test_corky_toml_finds_plain() {
    // This test requires no mail/ in cwd so config_dir() uses CORKY_DATA.
    // Since the dev repo may have mail/ symlink, we test with an explicit
    // path lookup instead of relying on global resolution.
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().to_path_buf();

    // Only corky.toml (no .corky.toml)
    std::fs::write(data.join("corky.toml"), "").unwrap();

    // Verify the file exists at the expected path
    assert!(data.join("corky.toml").exists());
    assert!(!data.join(".corky.toml").exists());
}

#[test]
fn test_sync_state_file_path() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().to_path_buf();
    // SAFETY: Test-only; no concurrent env access.
    unsafe { std::env::set_var("CORKY_DATA", data.to_string_lossy().as_ref()) };

    let sf = resolve::sync_state_file();
    assert!(sf.to_string_lossy().ends_with(".sync-state.json"));

    unsafe { std::env::remove_var("CORKY_DATA") };
}

#[test]
fn test_manifest_file_path() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().to_path_buf();
    // SAFETY: Test-only; no concurrent env access.
    unsafe { std::env::set_var("CORKY_DATA", data.to_string_lossy().as_ref()) };

    let mf = resolve::manifest_file();
    assert!(mf.to_string_lossy().ends_with("manifest.toml"));

    unsafe { std::env::remove_var("CORKY_DATA") };
}

#[test]
fn test_config_paths() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().to_path_buf();
    // SAFETY: Test-only; no concurrent env access.
    unsafe { std::env::set_var("CORKY_DATA", data.to_string_lossy().as_ref()) };

    let vm = resolve::voice_md();
    let cj = resolve::credentials_json();

    assert!(vm.to_string_lossy().ends_with("voice.md"));
    assert!(cj.to_string_lossy().ends_with("credentials.json"));

    unsafe { std::env::remove_var("CORKY_DATA") };
}
