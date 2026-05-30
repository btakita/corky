//! App-level config for corky (mailbox registry, defaults).
//!
//! Reads/writes {user_config_dir}/corky/config.toml.

use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::resolve;

/// Return the OS-native corky config directory.
pub fn app_config_dir() -> PathBuf {
    if let Some(proj_dirs) = directories::ProjectDirs::from("", "", "corky") {
        proj_dirs.config_dir().to_path_buf()
    } else {
        resolve::home_dir().join(".config").join("corky")
    }
}

/// Return the path to config.toml.
pub fn app_config_path() -> PathBuf {
    app_config_dir().join("config.toml")
}

/// Read config.toml, returning empty table if missing.
pub fn load() -> Result<toml::Value> {
    let path = app_config_path();
    if !path.exists() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    let content = std::fs::read_to_string(&path)?;
    let val: toml::Value = toml::from_str(&content)?;
    Ok(val)
}

/// Write config.toml, creating parent dir if needed.
pub fn save(config: &toml::Value) -> Result<()> {
    let path = app_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

fn read_mailboxes(
    table: &toml::map::Map<String, toml::Value>,
) -> toml::map::Map<String, toml::Value> {
    if let Some(toml::Value::Table(m)) = table.get("mailboxes") {
        return m.clone();
    }
    toml::map::Map::new()
}

fn read_default(table: &toml::map::Map<String, toml::Value>) -> Option<String> {
    if let Some(toml::Value::String(d)) = table.get("default_mailbox") {
        return Some(d.clone());
    }
    None
}

/// Resolve a mailbox name to a data directory path.
///
/// - If name given: look up, error if not found.
/// - No name + default_mailbox set: use default.
/// - No name + exactly 1 mailbox: use it implicitly.
/// - No name + multiple mailboxes, no default: error with list.
/// - No mailboxes configured: return None.
pub fn resolve_mailbox(name: Option<&str>) -> Result<Option<PathBuf>> {
    let config = load()?;
    let table = config.as_table().cloned().unwrap_or_default();
    let mailboxes = read_mailboxes(&table);

    if mailboxes.is_empty() {
        return Ok(None);
    }

    if let Some(name) = name {
        match mailboxes.get(name) {
            Some(mailbox_val) => {
                let path = mailbox_path(mailbox_val)?;
                return Ok(Some(path));
            }
            None => {
                let available: Vec<&String> = mailboxes.keys().collect();
                bail!(
                    "Unknown mailbox '{}'. Available: {}",
                    name,
                    available
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }

    // No name given — try defaults
    if let Some(default) = read_default(&table)
        && let Some(mailbox_val) = mailboxes.get(default.as_str())
    {
        let path = mailbox_path(mailbox_val)?;
        return Ok(Some(path));
    }

    if mailboxes.len() == 1 {
        let (_, mailbox_val) = mailboxes.iter().next().unwrap();
        let path = mailbox_path(mailbox_val)?;
        return Ok(Some(path));
    }

    // Multiple mailboxes, no default
    eprintln!("Multiple mailboxes configured. Use --mailbox NAME or set default_mailbox.");
    eprintln!();
    for (mname, mconf) in &mailboxes {
        if let Some(p) = mconf.get("path").and_then(|v| v.as_str()) {
            eprintln!("  {}  {}", mname, p);
        }
    }
    std::process::exit(1);
}

/// Register a mailbox, auto-default if first.
pub fn add_mailbox(name: &str, path: &str) -> Result<()> {
    let mut config = load()?;
    let table = config.as_table_mut().unwrap();

    let mailboxes = table
        .entry("mailboxes")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .unwrap();

    let mut mailbox_entry = toml::map::Map::new();
    mailbox_entry.insert("path".to_string(), toml::Value::String(path.to_string()));
    mailboxes.insert(name.to_string(), toml::Value::Table(mailbox_entry));

    if mailboxes.len() == 1 {
        table.insert(
            "default_mailbox".to_string(),
            toml::Value::String(name.to_string()),
        );
    }

    save(&config)
}

/// List all configured mailboxes as (name, path, is_default).
pub fn list_mailboxes() -> Result<Vec<(String, String, bool)>> {
    let config = load()?;
    let table = config.as_table().cloned().unwrap_or_default();
    let default = read_default(&table).unwrap_or_default();

    let mailboxes = read_mailboxes(&table);

    let mut result = vec![];
    // Use BTreeMap for sorted output
    let sorted: BTreeMap<_, _> = mailboxes.into_iter().collect();
    for (name, val) in sorted {
        let path = val
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let is_default = name == default;
        result.push((name, path, is_default));
    }
    Ok(result)
}

fn mailbox_path(mailbox_val: &toml::Value) -> Result<PathBuf> {
    let path_str = mailbox_val
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Ok(resolve::expand_tilde(path_str))
}
