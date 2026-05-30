//! Contact configuration — parse [contacts.*] from .corky.toml.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::resolve;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Contact {
    #[serde(default)]
    pub emails: Vec<String>,
    /// Explicitly share this contact with these mailboxes (even without conversation match).
    #[serde(default)]
    pub shared_with: Vec<String>,
    /// Aliases for matching sender names that don't slugify to the directory name.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Load contacts from [contacts.*] in .corky.toml and return {name: Contact} mapping.
pub fn load_contacts(path: Option<&Path>) -> Result<BTreeMap<String, Contact>> {
    let path = path.map(PathBuf::from).unwrap_or_else(resolve::corky_toml);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = std::fs::read_to_string(&path)?;
    if content.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let raw: toml::Value = toml::from_str(&content)?;
    let contacts_table = raw
        .as_table()
        .and_then(|t| t.get("contacts"))
        .and_then(|v| v.as_table());
    match contacts_table {
        Some(table) => {
            let mut result = BTreeMap::new();
            for (name, data) in table {
                let contact: Contact = data.clone().try_into()?;
                result.insert(name.clone(), contact);
            }
            Ok(result)
        }
        None => Ok(BTreeMap::new()),
    }
}

/// Write a single contact to [contacts.{name}] in .corky.toml (format-preserving).
///
/// Not concurrency-safe: uses read-modify-write without file locking.
/// This is fine for a single-user CLI — the only concurrent scenario would
/// be `corky watch` (background) while the user runs another command, but
/// watch only reads config, never writes. If corky ever becomes multi-process
/// with concurrent writers, add file locking here and in the other
/// .corky.toml writers (accounts::add_label_to_account, mailbox add/remove/rename).
pub fn save_contact(name: &str, contact: &Contact, path: Option<&Path>) -> Result<()> {
    let path = path.map(PathBuf::from).unwrap_or_else(resolve::corky_toml);
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    // Ensure [contacts] table exists
    if doc.get("contacts").is_none() {
        doc.insert("contacts", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let contacts = doc["contacts"].as_table_mut().unwrap();

    // Build contact table
    let mut table = toml_edit::Table::new();
    if !contact.emails.is_empty() {
        let mut arr = toml_edit::Array::new();
        for e in &contact.emails {
            arr.push(e.as_str());
        }
        table.insert("emails", toml_edit::value(arr));
    }
    if !contact.shared_with.is_empty() {
        let mut arr = toml_edit::Array::new();
        for s in &contact.shared_with {
            arr.push(s.as_str());
        }
        table.insert("shared_with", toml_edit::value(arr));
    }
    if !contact.aliases.is_empty() {
        let mut arr = toml_edit::Array::new();
        for a in &contact.aliases {
            arr.push(a.as_str());
        }
        table.insert("aliases", toml_edit::value(arr));
    }
    contacts.insert(name, toml_edit::Item::Table(table));

    std::fs::write(&path, doc.to_string())?;
    Ok(())
}
