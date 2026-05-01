//! `corky topics info` — show topic details and matching threads.

use anyhow::Result;

use crate::config::topic;
use crate::resolve;
use crate::util;

pub fn run(name: &str) -> Result<()> {
    let topics = topic::load_topics(None)?;
    let config = topics
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Topic '{}' not found in .corky.toml.", name))?;

    println!("Topic: {}", name);
    if let Some(ref desc) = config.description {
        println!("Description: {}", desc);
    }
    if !config.keywords.is_empty() {
        println!("Keywords: {}", config.keywords.join(", "));
    }
    if !config.contacts.is_empty() {
        println!("Contacts: {}", config.contacts.join(", "));
    }
    if !config.mailboxes.is_empty() {
        println!("Mailboxes: {}", config.mailboxes.join(", "));
    }

    // Find matching conversations
    if !config.keywords.is_empty() {
        let matches = find_matching_conversations(config)?;
        if matches.is_empty() {
            println!("\nNo matching conversations found.");
        } else {
            println!("\nMatching conversations ({}):", matches.len());
            for m in &matches {
                println!("  {}", m);
            }
        }
    }

    Ok(())
}

/// Scan conversations directory for threads whose subject matches any keyword.
fn find_matching_conversations(config: &topic::TopicConfig) -> Result<Vec<String>> {
    let conv_dir = resolve::conversations_dir();
    if !conv_dir.is_dir() {
        return Ok(Vec::new());
    }

    let lower_keywords: Vec<String> = config.keywords.iter().map(|k| k.to_lowercase()).collect();
    let mut matches = Vec::new();

    for entry in std::fs::read_dir(&conv_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let slug = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let subject = util::thread_key_from_subject(slug);
        if lower_keywords.iter().any(|kw| subject.contains(kw)) {
            matches.push(slug.to_string());
        }
    }

    matches.sort();
    Ok(matches)
}
