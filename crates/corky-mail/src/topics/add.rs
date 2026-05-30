//! `corky topics add` — add a topic to .corky.toml.

use anyhow::Result;

use crate::config::topic::{self, TopicConfig};

pub fn run(name: &str, keywords: &[String], description: Option<&str>) -> Result<()> {
    let existing = topic::load_topics(None)?;
    if existing.contains_key(name) {
        anyhow::bail!(
            "Topic '{}' already exists. Edit .corky.toml directly to update it.",
            name
        );
    }
    let config = TopicConfig {
        keywords: keywords.to_vec(),
        description: description.map(String::from),
        ..Default::default()
    };
    topic::save_topic(name, &config, None)?;
    println!("Added topic '{}'.", name);
    if !keywords.is_empty() {
        println!("  keywords: {}", keywords.join(", "));
    }
    if let Some(desc) = description {
        println!("  description: {}", desc);
    }
    Ok(())
}
