//! Account configuration — parse accounts.toml with provider presets.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::resolve;

/// Provider presets for common IMAP/SMTP configurations.
pub fn provider_presets() -> HashMap<&'static str, AccountDefaults> {
    let mut m = HashMap::new();
    m.insert(
        "gmail",
        AccountDefaults {
            imap_host: "imap.gmail.com",
            imap_port: 993,
            imap_starttls: false,
            smtp_host: "smtp.gmail.com",
            smtp_port: 465,
            drafts_folder: "[Gmail]/Drafts",
        },
    );
    m.insert(
        "protonmail-bridge",
        AccountDefaults {
            imap_host: "127.0.0.1",
            imap_port: 1143,
            imap_starttls: true,
            smtp_host: "127.0.0.1",
            smtp_port: 1025,
            drafts_folder: "Drafts",
        },
    );
    m
}

pub struct AccountDefaults {
    pub imap_host: &'static str,
    pub imap_port: u16,
    pub imap_starttls: bool,
    pub smtp_host: &'static str,
    pub smtp_port: u16,
    pub drafts_folder: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerConfig {
    pub github_user: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub auto_upgrade: bool,
}

fn default_poll_interval() -> u64 {
    300
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            poll_interval: 300,
            notify: false,
            auto_upgrade: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub password_cmd: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub imap_host: String,
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    #[serde(default)]
    pub imap_starttls: bool,
    #[serde(default)]
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    #[serde(default = "default_drafts_folder")]
    pub drafts_folder: String,
    #[serde(default = "default_sync_days")]
    pub sync_days: u32,
    #[serde(default)]
    pub default: bool,
}

fn default_provider() -> String {
    "imap".to_string()
}
fn default_imap_port() -> u16 {
    993
}
fn default_smtp_port() -> u16 {
    465
}
fn default_drafts_folder() -> String {
    "Drafts".to_string()
}
fn default_sync_days() -> u32 {
    3650
}

impl Default for Account {
    fn default() -> Self {
        Self {
            provider: "imap".to_string(),
            user: String::new(),
            password: String::new(),
            password_cmd: String::new(),
            labels: vec![],
            imap_host: String::new(),
            imap_port: 993,
            imap_starttls: false,
            smtp_host: String::new(),
            smtp_port: 465,
            drafts_folder: "Drafts".to_string(),
            sync_days: 3650,
            default: false,
        }
    }
}

/// Apply provider preset defaults. Account values win over preset.
fn apply_preset(account: &mut Account) {
    let presets = provider_presets();
    let Some(preset) = presets.get(account.provider.as_str()) else {
        return;
    };
    let defaults = Account::default();
    if account.imap_host == defaults.imap_host {
        account.imap_host = preset.imap_host.to_string();
    }
    if account.imap_port == defaults.imap_port {
        account.imap_port = preset.imap_port;
    }
    if account.imap_starttls == defaults.imap_starttls && preset.imap_starttls {
        account.imap_starttls = preset.imap_starttls;
    }
    if account.smtp_host == defaults.smtp_host {
        account.smtp_host = preset.smtp_host.to_string();
    }
    if account.smtp_port == defaults.smtp_port {
        account.smtp_port = preset.smtp_port;
    }
    if account.drafts_folder == defaults.drafts_folder {
        account.drafts_folder = preset.drafts_folder.to_string();
    }
}

/// Resolve password: inline value if set, else run password_cmd.
pub fn resolve_password(account: &Account) -> Result<String> {
    crate::util::resolve_secret(
        &account.password,
        &account.password_cmd,
        &format!("Account {:?} has no password or password_cmd", account.user),
    )
}

/// Parse accounts from .corky.toml → {name: Account} mapping.
pub fn load_accounts(path: Option<&Path>) -> Result<HashMap<String, Account>> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => resolve::corky_toml(),
    };
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content = std::fs::read_to_string(&path)?;
    let raw: toml::Value = toml::from_str(&content)?;
    let table = raw.as_table().unwrap();

    let Some(toml::Value::Table(accounts_section)) = table.get("accounts") else {
        return Ok(HashMap::new());
    };

    let mut result = HashMap::new();
    for (name, data) in accounts_section {
        if !data.is_table() {
            continue;
        }
        let mut account: Account = data.clone().try_into()?;
        apply_preset(&mut account);
        result.insert(name.clone(), account);
    }
    Ok(result)
}

/// Load [owner] section from .corky.toml.
pub fn load_owner(path: Option<&Path>) -> Result<OwnerConfig> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => resolve::corky_toml(),
    };
    if !path.exists() {
        bail!(
            "Config not found at {}.\nRun 'corky init' or add an [owner] section with github_user.",
            path.display()
        );
    }
    let content = std::fs::read_to_string(&path)?;
    let raw: toml::Value = toml::from_str(&content)?;
    let owner_data = raw
        .get("owner")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Missing [owner] section in config.\nAdd: [owner]\ngithub_user = \"your-github-username\""
            )
        })?;
    let owner: OwnerConfig = owner_data.clone().try_into()?;
    Ok(owner)
}

/// Return (name, account) for the default account.
pub fn get_default_account(accounts: &HashMap<String, Account>) -> Result<(String, Account)> {
    for (name, acct) in accounts {
        if acct.default {
            return Ok((name.clone(), acct.clone()));
        }
    }
    // Fall back to first account
    let (name, acct) = accounts.iter().next().ok_or_else(|| anyhow::anyhow!("No accounts configured"))?;
    Ok((name.clone(), acct.clone()))
}

/// Lookup account by email address.
pub fn get_account_for_email(
    accounts: &HashMap<String, Account>,
    email_addr: &str,
) -> Option<(String, Account)> {
    let email_lower = email_addr.to_lowercase();
    for (name, acct) in accounts {
        if acct.user.to_lowercase() == email_lower {
            return Some((name.clone(), acct.clone()));
        }
    }
    None
}

/// Add a label to an account's labels list in .corky.toml.
///
/// Uses toml_edit for format-preserving edits.
/// Returns Ok(true) if added, Ok(false) if already present.
pub fn add_label_to_account(account_name: &str, label: &str, path: Option<&Path>) -> Result<bool> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => resolve::corky_toml(),
    };
    if !path.exists() {
        bail!("Config not found at {}", path.display());
    }

    // Verify account exists and label isn't already there
    let accounts = load_accounts(Some(&path))?;
    let Some(acct) = accounts.get(account_name) else {
        bail!(
            "Unknown account: {}\nAvailable: {}",
            account_name,
            accounts.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    };
    if acct.labels.contains(&label.to_string()) {
        return Ok(false);
    }

    // Format-preserving edit with toml_edit
    let content = std::fs::read_to_string(&path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    let labels_array = doc.get_mut("accounts")
        .and_then(|t| t.get_mut(account_name))
        .and_then(|t| t.get_mut("labels"));

    if let Some(labels) = labels_array
        && let Some(arr) = labels.as_array_mut() {
            arr.push(label);
        }

    std::fs::write(&path, doc.to_string())?;
    Ok(true)
}

/// CLI: corky add-label LABEL --account ACCOUNT
pub fn add_label_cmd(label: &str, account: &str) -> Result<()> {
    let added = add_label_to_account(account, label, None)?;
    if added {
        println!("Added '{}' to account '{}'", label, account);
    } else {
        println!("Label '{}' already in account '{}'", label, account);
    }
    Ok(())
}

/// Load [watch] section from .corky.toml. Returns defaults if missing.
pub fn load_watch_config(path: Option<&Path>) -> Result<WatchConfig> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => resolve::corky_toml(),
    };
    if !path.exists() {
        return Ok(WatchConfig::default());
    }
    let content = std::fs::read_to_string(&path)?;
    let raw: toml::Value = toml::from_str(&content)?;
    match raw.get("watch") {
        Some(watch_data) => {
            let config: WatchConfig = watch_data.clone().try_into()?;
            Ok(config)
        }
        None => Ok(WatchConfig::default()),
    }
}
