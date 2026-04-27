//! Integration tests for account config parsing (src/accounts.rs).

mod common;

use std::collections::HashMap;
use tempfile::TempDir;

use corky::accounts::{
    self, Account, get_account_for_email, get_default_account, load_accounts, load_owner,
    load_watch_config, resolve_password,
};

#[test]
fn test_load_accounts_nested_format() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("accounts.toml");
    std::fs::write(
        &path,
        r#"
[owner]
github_user = "testuser"
name = "Test User"

[accounts.personal]
provider = "gmail"
user = "alice@gmail.com"
password = "secret123"
labels = ["inbox"]
default = true

[accounts.work]
provider = "imap"
user = "bob@work.com"
password = "workpwd"
imap_host = "mail.work.com"
smtp_host = "smtp.work.com"
labels = ["work-label"]
"#,
    )
    .unwrap();

    let accounts = load_accounts(Some(&path)).unwrap();
    assert_eq!(accounts.len(), 2);
    assert!(accounts.contains_key("personal"));
    assert!(accounts.contains_key("work"));

    let work = accounts.get("work").unwrap();
    assert_eq!(work.imap_host, "mail.work.com");
    assert_eq!(work.smtp_host, "smtp.work.com");
}

#[test]
fn test_load_accounts_missing_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nonexistent.toml");

    let accounts = load_accounts(Some(&path)).unwrap();
    assert!(accounts.is_empty());
}

#[test]
fn test_load_accounts_empty_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("accounts.toml");
    std::fs::write(&path, "").unwrap();

    let accounts = load_accounts(Some(&path)).unwrap();
    assert!(accounts.is_empty());
}

#[test]
fn test_provider_preset_gmail() {
    let presets = accounts::provider_presets();
    let gmail = presets.get("gmail").unwrap();
    assert_eq!(gmail.imap_host, "imap.gmail.com");
    assert_eq!(gmail.imap_port, 993);
    assert!(!gmail.imap_starttls);
    assert_eq!(gmail.smtp_host, "smtp.gmail.com");
    assert_eq!(gmail.smtp_port, 465);
    assert_eq!(gmail.drafts_folder, "[Gmail]/Drafts");
}

#[test]
fn test_provider_preset_protonmail() {
    let presets = accounts::provider_presets();
    let pm = presets.get("protonmail-bridge").unwrap();
    assert_eq!(pm.imap_host, "127.0.0.1");
    assert_eq!(pm.imap_port, 1143);
    assert!(pm.imap_starttls);
    assert_eq!(pm.smtp_host, "127.0.0.1");
    assert_eq!(pm.smtp_port, 1025);
    assert_eq!(pm.drafts_folder, "Drafts");
}

#[test]
fn test_resolve_password_inline() {
    let acct = Account {
        password: "mypassword".to_string(),
        ..Default::default()
    };
    let pwd = resolve_password(&acct).unwrap();
    assert_eq!(pwd, "mypassword");
}

#[test]
fn test_resolve_password_cmd() {
    let acct = Account {
        password_cmd: "echo secretfromcmd".to_string(),
        ..Default::default()
    };
    let pwd = resolve_password(&acct).unwrap();
    assert_eq!(pwd, "secretfromcmd");
}

#[test]
fn test_resolve_password_missing() {
    let acct = Account {
        user: "test@example.com".to_string(),
        ..Default::default()
    };
    let result = resolve_password(&acct);
    assert!(result.is_err());
}

#[test]
fn test_get_default_account() {
    let mut accounts = HashMap::new();
    accounts.insert(
        "personal".to_string(),
        Account {
            user: "personal@example.com".to_string(),
            default: true,
            ..Default::default()
        },
    );
    accounts.insert(
        "work".to_string(),
        Account {
            user: "work@example.com".to_string(),
            ..Default::default()
        },
    );

    let (name, acct) = get_default_account(&accounts).unwrap();
    assert_eq!(name, "personal");
    assert_eq!(acct.user, "personal@example.com");
}

#[test]
fn test_get_default_account_fallback() {
    let mut accounts = HashMap::new();
    accounts.insert(
        "only".to_string(),
        Account {
            user: "only@example.com".to_string(),
            ..Default::default()
        },
    );

    let (name, acct) = get_default_account(&accounts).unwrap();
    assert_eq!(name, "only");
    assert_eq!(acct.user, "only@example.com");
}

#[test]
fn test_get_default_account_empty() {
    let accounts: HashMap<String, Account> = HashMap::new();
    let result = get_default_account(&accounts);
    assert!(result.is_err());
}

#[test]
fn test_get_account_for_email() {
    let mut accounts = HashMap::new();
    accounts.insert(
        "personal".to_string(),
        Account {
            user: "alice@gmail.com".to_string(),
            ..Default::default()
        },
    );
    accounts.insert(
        "work".to_string(),
        Account {
            user: "alice@work.com".to_string(),
            ..Default::default()
        },
    );

    let result = get_account_for_email(&accounts, "alice@work.com");
    assert!(result.is_some());
    let (name, acct) = result.unwrap();
    assert_eq!(name, "work");
    assert_eq!(acct.user, "alice@work.com");
}

#[test]
fn test_get_account_for_email_case_insensitive() {
    let mut accounts = HashMap::new();
    accounts.insert(
        "personal".to_string(),
        Account {
            user: "Alice@Gmail.com".to_string(),
            ..Default::default()
        },
    );

    let result = get_account_for_email(&accounts, "alice@gmail.com");
    assert!(result.is_some());
}

#[test]
fn test_get_account_for_email_not_found() {
    let mut accounts = HashMap::new();
    accounts.insert(
        "personal".to_string(),
        Account {
            user: "alice@gmail.com".to_string(),
            ..Default::default()
        },
    );

    let result = get_account_for_email(&accounts, "nobody@gmail.com");
    assert!(result.is_none());
}

#[test]
fn test_load_owner() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("accounts.toml");
    std::fs::write(
        &path,
        r#"
[owner]
github_user = "myghuser"
name = "My Name"
"#,
    )
    .unwrap();

    let owner = load_owner(Some(&path)).unwrap();
    assert_eq!(owner.github_user, "myghuser");
    assert_eq!(owner.name, "My Name");
}

#[test]
fn test_load_owner_missing_section() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".corky.toml");
    std::fs::write(
        &path,
        r#"
[accounts.personal]
provider = "gmail"
"#,
    )
    .unwrap();

    let result = load_owner(Some(&path));
    assert!(result.is_err());
}

#[test]
fn test_load_owner_missing_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nonexistent.toml");

    let result = load_owner(Some(&path));
    assert!(result.is_err());
}

#[test]
fn test_load_watch_config_defaults() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("accounts.toml");
    std::fs::write(&path, "").unwrap();

    let wc = load_watch_config(Some(&path)).unwrap();
    assert_eq!(wc.poll_interval, 300);
    assert!(!wc.notify);
}

#[test]
fn test_load_watch_config_custom() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("accounts.toml");
    std::fs::write(
        &path,
        r#"
[watch]
poll_interval = 60
notify = true
"#,
    )
    .unwrap();

    let wc = load_watch_config(Some(&path)).unwrap();
    assert_eq!(wc.poll_interval, 60);
    assert!(wc.notify);
}

#[test]
fn test_load_watch_config_missing_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nonexistent.toml");

    let wc = load_watch_config(Some(&path)).unwrap();
    assert_eq!(wc.poll_interval, 300);
    assert!(!wc.notify);
}

#[test]
fn test_account_defaults() {
    let acct = Account::default();
    assert_eq!(acct.provider, "imap");
    assert!(acct.user.is_empty());
    assert!(acct.password.is_empty());
    assert!(acct.password_cmd.is_empty());
    assert!(acct.labels.is_empty());
    assert_eq!(acct.imap_port, 993);
    assert!(!acct.imap_starttls);
    assert_eq!(acct.smtp_port, 465);
    assert_eq!(acct.drafts_folder, "Drafts");
    assert_eq!(acct.sync_days, 3650);
    assert!(!acct.default);
}

#[test]
fn test_preset_overrides_only_defaults() {
    // If account specifies imap_host, preset should not override it
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".corky.toml");
    std::fs::write(
        &path,
        r#"
[accounts.personal]
provider = "gmail"
user = "alice@gmail.com"
password = "test"
imap_host = "custom.imap.host"
"#,
    )
    .unwrap();

    let accounts = load_accounts(Some(&path)).unwrap();
    let acct = accounts.get("personal").unwrap();
    // Custom host should be preserved
    assert_eq!(acct.imap_host, "custom.imap.host");
    // But smtp_host should get the preset since it wasn't overridden
    assert_eq!(acct.smtp_host, "smtp.gmail.com");
}

#[test]
fn test_add_label_to_account() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".corky.toml");
    std::fs::write(
        &path,
        r#"
[accounts.personal]
provider = "gmail"
user = "alice@gmail.com"
password = "test"
labels = ["inbox"]
"#,
    )
    .unwrap();

    let added = accounts::add_label_to_account("personal", "new-label", Some(&path)).unwrap();
    assert!(added);

    // Verify the label was actually added
    let accounts = load_accounts(Some(&path)).unwrap();
    let acct = accounts.get("personal").unwrap();
    assert!(acct.labels.contains(&"new-label".to_string()));
}

#[test]
fn test_add_label_already_exists() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".corky.toml");
    std::fs::write(
        &path,
        r#"
[accounts.personal]
provider = "gmail"
user = "alice@gmail.com"
password = "test"
labels = ["inbox"]
"#,
    )
    .unwrap();

    let added = accounts::add_label_to_account("personal", "inbox", Some(&path)).unwrap();
    assert!(!added);
}

#[test]
fn test_add_label_unknown_account() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".corky.toml");
    std::fs::write(
        &path,
        r#"
[accounts.personal]
provider = "gmail"
user = "alice@gmail.com"
password = "test"
labels = ["inbox"]
"#,
    )
    .unwrap();

    let result = accounts::add_label_to_account("nonexistent", "label", Some(&path));
    assert!(result.is_err());
}

#[test]
fn test_protonmail_bridge_preset() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".corky.toml");
    std::fs::write(
        &path,
        r#"
[accounts.proton]
provider = "protonmail-bridge"
user = "user@proton.me"
password = "bridge-password"
labels = ["inbox"]
"#,
    )
    .unwrap();

    let accounts = load_accounts(Some(&path)).unwrap();
    let acct = accounts.get("proton").unwrap();
    assert_eq!(acct.imap_host, "127.0.0.1");
    assert_eq!(acct.imap_port, 1143);
    assert!(acct.imap_starttls);
    assert_eq!(acct.smtp_host, "127.0.0.1");
    assert_eq!(acct.smtp_port, 1025);
    assert_eq!(acct.drafts_folder, "Drafts");
}
