//! manifest.toml generation from conversation files + .corky.toml contacts.

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;

use super::markdown::parse_thread_markdown;
use crate::config::contact;

static EMAIL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<([^>]+)>").unwrap());

/// Generate manifest.toml from conversation files + .corky.toml contacts.
pub fn generate_manifest(conversations_dir: &Path) -> Result<()> {
    if !conversations_dir.exists() {
        return Ok(());
    }

    let contacts = contact::load_contacts(None).unwrap_or_default();

    // Build email→contact-name lookup
    let mut email_to_contact: BTreeMap<String, String> = BTreeMap::new();
    for (cname, c) in &contacts {
        for addr in &c.emails {
            email_to_contact.insert(addr.to_lowercase(), cname.clone());
        }
    }

    let mut threads: BTreeMap<String, toml::Value> = BTreeMap::new();

    let mut entries: Vec<_> = std::fs::read_dir(conversations_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext == "md")
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let text = std::fs::read_to_string(&path)?;
        let thread = match parse_thread_markdown(&text) {
            Some(t) => t,
            None => continue,
        };

        // Match contacts by email addresses in from, to, and cc
        let mut thread_contacts: Vec<String> = Vec::new();
        for msg in &thread.messages {
            for field in [&msg.from, &msg.to, &msg.cc] {
                for cap in EMAIL_RE.captures_iter(field) {
                    let addr = cap[1].to_lowercase();
                    if let Some(cname) = email_to_contact.get(&addr)
                        && !thread_contacts.contains(cname)
                    {
                        thread_contacts.push(cname.clone());
                    }
                }
            }
        }

        let slug = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mut entry_map = toml::map::Map::new();
        entry_map.insert("subject".to_string(), toml::Value::String(thread.subject));
        entry_map.insert("thread_id".to_string(), toml::Value::String(thread.id));
        entry_map.insert(
            "labels".to_string(),
            toml::Value::Array(thread.labels.into_iter().map(toml::Value::String).collect()),
        );
        entry_map.insert(
            "accounts".to_string(),
            toml::Value::Array(
                thread
                    .accounts
                    .into_iter()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );
        entry_map.insert(
            "last_updated".to_string(),
            toml::Value::String(thread.last_date),
        );
        entry_map.insert(
            "contacts".to_string(),
            toml::Value::Array(
                thread_contacts
                    .into_iter()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );

        threads.insert(slug, toml::Value::Table(entry_map));
    }

    let manifest_path = conversations_dir
        .parent()
        .unwrap_or(conversations_dir)
        .join("manifest.toml");
    let mut manifest = toml::map::Map::new();
    let threads_table: toml::map::Map<String, toml::Value> = threads.into_iter().collect();
    manifest.insert("threads".to_string(), toml::Value::Table(threads_table));
    let content = toml::to_string_pretty(&toml::Value::Table(manifest))?;
    std::fs::write(&manifest_path, content)?;
    println!("  Generated {}", manifest_path.display());
    Ok(())
}
