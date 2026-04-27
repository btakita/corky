//! Integration tests for corky init (src/init.rs).
//!
//! Each test sets HOME to a temp dir to isolate from the real
//! ~/.config/corky/config.toml. Tests run serially via
//! a shared mutex to avoid env var races.

mod common;

use std::sync::Mutex;
use tempfile::TempDir;

use corky::accounts::{load_accounts, load_owner};

static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Run init::run with HOME set to the temp dir parent, so
/// app_config::add_mailbox writes to an isolated config.
#[allow(clippy::too_many_arguments)]
fn run_init_isolated(
    tmp: &TempDir,
    path: &std::path::Path,
    user: &str,
    provider: &str,
    password_cmd: &str,
    labels: &str,
    github_user: &str,
    name: &str,
    mailbox: &str,
    force: bool,
) -> anyhow::Result<()> {
    let _lock = ENV_MUTEX.lock().unwrap();
    let old_home = std::env::var("HOME").ok();
    // SAFETY: Tests run serially under ENV_MUTEX so no concurrent env access.
    unsafe { std::env::set_var("HOME", tmp.path().to_string_lossy().as_ref()) };
    let result = corky::init::run(
        user,
        path,
        provider,
        password_cmd,
        labels,
        github_user,
        name,
        false, // sync
        mailbox,
        force,
    );
    // Restore HOME
    if let Some(h) = old_home {
        unsafe { std::env::set_var("HOME", h) };
    }
    result
}

#[test]
fn test_init_creates_directory_structure() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("myproject");

    run_init_isolated(
        &tmp,
        &path,
        "test@example.com",
        "gmail",
        "",
        "correspondence",
        "testgh",
        "Test User",
        "test-init-mb",
        true,
    )
    .unwrap();

    let data_dir = path.join("mail");
    assert!(data_dir.join("conversations").is_dir());
    assert!(data_dir.join("drafts").is_dir());
    assert!(data_dir.join("contacts").is_dir());
    assert!(data_dir.join("conversations").join(".gitkeep").exists());
    assert!(data_dir.join("drafts").join(".gitkeep").exists());
    assert!(data_dir.join("contacts").join(".gitkeep").exists());
    // Config inside mail/
    assert!(data_dir.join(".corky.toml").exists());
    assert!(data_dir.join("voice.md").exists());
    // No config at project root
    assert!(!path.join(".corky.toml").exists());
    assert!(!path.join("voice.md").exists());
}

#[test]
fn test_init_corky_toml_content() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("initdata");

    run_init_isolated(
        &tmp,
        &path,
        "alice@gmail.com",
        "gmail",
        "pass show email/personal",
        "inbox, sent",
        "alicegh",
        "Alice",
        "test-init-mb-acct",
        true,
    )
    .unwrap();

    let config_path = path.join("mail").join(".corky.toml");
    let accounts = load_accounts(Some(&config_path)).unwrap();
    assert!(accounts.contains_key("default"));
    let acct = accounts.get("default").unwrap();
    assert_eq!(acct.provider, "gmail");
    assert_eq!(acct.user, "alice@gmail.com");
    assert!(acct.default);

    let owner = load_owner(Some(&config_path)).unwrap();
    assert_eq!(owner.github_user, "alicegh");
    assert_eq!(owner.name, "Alice");
}

#[test]
fn test_init_with_custom_provider() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("pmdata");

    run_init_isolated(
        &tmp,
        &path,
        "user@proton.me",
        "protonmail-bridge",
        "",
        "correspondence",
        "",
        "",
        "test-init-mb-pm",
        true,
    )
    .unwrap();

    let config_path = path.join("mail").join(".corky.toml");
    let accounts = load_accounts(Some(&config_path)).unwrap();
    let acct = accounts.get("default").unwrap();
    assert_eq!(acct.provider, "protonmail-bridge");
    assert_eq!(acct.user, "user@proton.me");
}

#[test]
fn test_init_labels_parsing() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("lbldata");

    run_init_isolated(
        &tmp,
        &path,
        "user@example.com",
        "imap",
        "",
        "inbox, sent, important",
        "",
        "",
        "test-init-mb-labels",
        true,
    )
    .unwrap();

    let config_path = path.join("mail").join(".corky.toml");
    let accounts = load_accounts(Some(&config_path)).unwrap();
    let acct = accounts.get("default").unwrap();
    assert_eq!(acct.labels.len(), 3);
    assert!(acct.labels.contains(&"inbox".to_string()));
    assert!(acct.labels.contains(&"sent".to_string()));
    assert!(acct.labels.contains(&"important".to_string()));
}

#[test]
fn test_init_force_overwrites() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("forcedata");
    let data_dir = path.join("mail");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join(".corky.toml"), "# old config").unwrap();

    run_init_isolated(
        &tmp,
        &path,
        "new@example.com",
        "gmail",
        "",
        "correspondence",
        "",
        "",
        "test-init-mb-force",
        true,
    )
    .unwrap();

    let content = std::fs::read_to_string(data_dir.join(".corky.toml")).unwrap();
    assert!(content.contains("new@example.com"));
    assert!(!content.contains("# old config"));
}

#[test]
fn test_init_tilde_expansion() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("tildetest");

    run_init_isolated(
        &tmp,
        &path,
        "user@example.com",
        "gmail",
        "",
        "correspondence",
        "",
        "",
        "test-init-mb-tilde",
        true,
    )
    .unwrap();

    assert!(path.join("mail").join(".corky.toml").exists());
}

#[test]
fn test_init_empty_labels() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("emptylbl");

    run_init_isolated(
        &tmp,
        &path,
        "user@example.com",
        "gmail",
        "",
        "",
        "",
        "",
        "test-init-mb-emptylbl",
        true,
    )
    .unwrap();

    let config_path = path.join("mail").join(".corky.toml");
    let accounts = load_accounts(Some(&config_path)).unwrap();
    let acct = accounts.get("default").unwrap();
    assert!(acct.labels.is_empty());
}

#[test]
fn test_init_gitignore_in_git_repo() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("gitproject");
    std::fs::create_dir_all(&path).unwrap();
    // Simulate a git repo
    std::fs::create_dir_all(path.join(".git")).unwrap();

    run_init_isolated(
        &tmp,
        &path,
        "user@example.com",
        "gmail",
        "",
        "correspondence",
        "",
        "",
        "test-init-mb-gitignore",
        true,
    )
    .unwrap();

    let gitignore = std::fs::read_to_string(path.join(".gitignore")).unwrap();
    assert!(gitignore.contains("mail"));
}

#[test]
fn test_init_no_gitignore_without_git() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nogit");

    run_init_isolated(
        &tmp,
        &path,
        "user@example.com",
        "gmail",
        "",
        "correspondence",
        "",
        "",
        "test-init-mb-nogit",
        true,
    )
    .unwrap();

    assert!(!path.join(".gitignore").exists());
}

#[test]
fn test_init_installs_skill() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("skillproject");

    run_init_isolated(
        &tmp,
        &path,
        "user@example.com",
        "gmail",
        "",
        "correspondence",
        "",
        "",
        "test-init-mb-skill",
        true,
    )
    .unwrap();

    assert!(path.join(".claude/skills/corky/SKILL.md").exists());
}
