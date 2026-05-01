//! `corky topics list` — show configured topics.

use anyhow::Result;

use crate::config::topic;

pub fn run(verbose: bool) -> Result<()> {
    let topics = topic::load_topics(None)?;
    if topics.is_empty() {
        println!("No topics configured. Use 'corky topics add' to create one.");
        return Ok(());
    }
    for (name, config) in &topics {
        if verbose {
            println!("{}:", name);
            if let Some(ref desc) = config.description {
                println!("  description: {}", desc);
            }
            if !config.keywords.is_empty() {
                println!("  keywords: {}", config.keywords.join(", "));
            }
            if !config.contacts.is_empty() {
                println!("  contacts: {}", config.contacts.join(", "));
            }
            if !config.mailboxes.is_empty() {
                println!("  mailboxes: {}", config.mailboxes.join(", "));
            }
        } else {
            let desc = config.description.as_deref().unwrap_or("");
            if desc.is_empty() {
                println!("{}", name);
            } else {
                println!("{} — {}", name, desc);
            }
        }
    }
    Ok(())
}
