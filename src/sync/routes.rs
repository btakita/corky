//! Apply routing rules to existing conversations.

use anyhow::Result;

use super::imap_sync::build_label_routes;
use super::markdown::parse_thread_markdown;
use crate::resolve;

/// Apply `[routing]` rules to conversations already on disk.
///
/// Scans `conversations/*.md`, checks each thread's labels against the
/// routing table, and copies matching files into the corresponding
/// mailbox `conversations/` directories.
pub fn run() -> Result<()> {
    let routes = build_label_routes("");
    if routes.is_empty() {
        println!("No routing rules configured in .corky.toml");
        return Ok(());
    }

    let conv_dir = resolve::conversations_dir();
    if !conv_dir.exists() {
        anyhow::bail!("Conversations directory not found: {}", conv_dir.display());
    }

    let mut copied = 0u32;
    let mut skipped = 0u32;

    for entry in std::fs::read_dir(&conv_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let text = std::fs::read_to_string(&path)?;
        let thread = match parse_thread_markdown(&text) {
            Some(t) => t,
            None => {
                skipped += 1;
                continue;
            }
        };

        let filename = match path.file_name() {
            Some(f) => f,
            None => continue,
        };

        for label in &thread.labels {
            if let Some(dest_dirs) = routes.get(label) {
                for dest_dir in dest_dirs {
                    std::fs::create_dir_all(dest_dir)?;
                    let dest = dest_dir.join(filename);
                    std::fs::copy(&path, &dest)?;
                    println!("  {} -> {}", filename.to_string_lossy(), dest_dir.display());
                    copied += 1;
                }
            }
        }
    }

    if skipped > 0 {
        println!("Skipped {} unparseable file(s)", skipped);
    }
    println!("Routing complete: {} file(s) copied", copied);
    Ok(())
}
