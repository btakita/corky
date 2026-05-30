//! Topic configuration — parse [topics.*] from .corky.toml.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::resolve;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TopicConfig {
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub mailboxes: Vec<String>,
    #[serde(default)]
    pub contacts: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Load topics from [topics.*] in .corky.toml and return {name: TopicConfig} mapping.
pub fn load_topics(path: Option<&Path>) -> Result<BTreeMap<String, TopicConfig>> {
    let path = path.map(PathBuf::from).unwrap_or_else(resolve::corky_toml);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = std::fs::read_to_string(&path)?;
    if content.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let raw: toml::Value = toml::from_str(&content)?;
    let topics_table = raw
        .as_table()
        .and_then(|t| t.get("topics"))
        .and_then(|v| v.as_table());
    match topics_table {
        Some(table) => {
            let mut result = BTreeMap::new();
            for (name, data) in table {
                let topic: TopicConfig = data.clone().try_into()?;
                result.insert(name.clone(), topic);
            }
            Ok(result)
        }
        None => Ok(BTreeMap::new()),
    }
}

/// Return topics whose `mailboxes` field contains the given mailbox name.
pub fn topics_for_mailbox(
    mailbox: &str,
    path: Option<&Path>,
) -> Result<BTreeMap<String, TopicConfig>> {
    let all = load_topics(path)?;
    Ok(all
        .into_iter()
        .filter(|(_, t)| t.mailboxes.iter().any(|m| m == mailbox))
        .collect())
}

/// Write a single topic to [topics.{name}] in .corky.toml (format-preserving).
pub fn save_topic(name: &str, topic: &TopicConfig, path: Option<&Path>) -> Result<()> {
    let path = path.map(PathBuf::from).unwrap_or_else(resolve::corky_toml);
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    // Ensure [topics] table exists
    if doc.get("topics").is_none() {
        doc.insert("topics", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let topics = doc["topics"].as_table_mut().unwrap();

    // Build topic table
    let mut table = toml_edit::Table::new();
    if !topic.keywords.is_empty() {
        let mut arr = toml_edit::Array::new();
        for k in &topic.keywords {
            arr.push(k.as_str());
        }
        table.insert("keywords", toml_edit::value(arr));
    }
    if !topic.mailboxes.is_empty() {
        let mut arr = toml_edit::Array::new();
        for m in &topic.mailboxes {
            arr.push(m.as_str());
        }
        table.insert("mailboxes", toml_edit::value(arr));
    }
    if !topic.contacts.is_empty() {
        let mut arr = toml_edit::Array::new();
        for c in &topic.contacts {
            arr.push(c.as_str());
        }
        table.insert("contacts", toml_edit::value(arr));
    }
    if let Some(ref desc) = topic.description {
        table.insert("description", toml_edit::value(desc.as_str()));
    }
    topics.insert(name, toml_edit::Item::Table(table));

    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".corky.toml");
        std::fs::write(&path, "").unwrap();
        let topics = load_topics(Some(&path)).unwrap();
        assert!(topics.is_empty());
    }

    #[test]
    fn load_no_topics_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".corky.toml");
        std::fs::write(&path, "[owner]\nname = \"test\"\n").unwrap();
        let topics = load_topics(Some(&path)).unwrap();
        assert!(topics.is_empty());
    }

    #[test]
    fn load_topics_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".corky.toml");
        std::fs::write(
            &path,
            r#"
[topics.project-alpha]
keywords = ["alpha", "release"]
mailboxes = ["work"]
contacts = ["alice", "bob"]
description = "Alpha project"
"#,
        )
        .unwrap();
        let topics = load_topics(Some(&path)).unwrap();
        assert_eq!(topics.len(), 1);
        let alpha = &topics["project-alpha"];
        assert_eq!(alpha.keywords, vec!["alpha", "release"]);
        assert_eq!(alpha.mailboxes, vec!["work"]);
        assert_eq!(alpha.contacts, vec!["alice", "bob"]);
        assert_eq!(alpha.description.as_deref(), Some("Alpha project"));
    }

    #[test]
    fn save_topic_creates_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".corky.toml");
        std::fs::write(&path, "[owner]\nname = \"test\"\n").unwrap();
        let topic = TopicConfig {
            keywords: vec!["rust".into(), "dev".into()],
            contacts: vec!["alice".into()],
            ..Default::default()
        };
        save_topic("rust-dev", &topic, Some(&path)).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[owner]"), "preserves existing sections");
        assert!(content.contains("[topics.rust-dev]"));
        assert!(content.contains("\"rust\""));

        // Verify roundtrip
        let loaded = load_topics(Some(&path)).unwrap();
        assert_eq!(loaded["rust-dev"].keywords, vec!["rust", "dev"]);
        assert_eq!(loaded["rust-dev"].contacts, vec!["alice"]);
    }

    #[test]
    fn topics_for_mailbox_filters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".corky.toml");
        std::fs::write(
            &path,
            r#"
[topics.alpha]
keywords = ["a"]
mailboxes = ["lucas", "alice"]

[topics.beta]
keywords = ["b"]
mailboxes = ["alice"]

[topics.gamma]
keywords = ["c"]
mailboxes = ["lucas"]
"#,
        )
        .unwrap();

        let lucas_topics = topics_for_mailbox("lucas", Some(&path)).unwrap();
        assert_eq!(lucas_topics.len(), 2);
        assert!(lucas_topics.contains_key("alpha"));
        assert!(lucas_topics.contains_key("gamma"));
        assert!(!lucas_topics.contains_key("beta"));

        let alice_topics = topics_for_mailbox("alice", Some(&path)).unwrap();
        assert_eq!(alice_topics.len(), 2);
        assert!(alice_topics.contains_key("alpha"));
        assert!(alice_topics.contains_key("beta"));

        let nobody_topics = topics_for_mailbox("nobody", Some(&path)).unwrap();
        assert!(nobody_topics.is_empty());
    }

    #[test]
    fn save_topic_preserves_existing_topics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".corky.toml");
        std::fs::write(
            &path,
            r#"[topics.existing]
keywords = ["keep"]
"#,
        )
        .unwrap();
        let topic = TopicConfig {
            keywords: vec!["new".into()],
            ..Default::default()
        };
        save_topic("added", &topic, Some(&path)).unwrap();

        let loaded = load_topics(Some(&path)).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["existing"].keywords, vec!["keep"]);
        assert_eq!(loaded["added"].keywords, vec!["new"]);
    }
}
