//! `corky topics suggest` — auto-discover topic candidates from conversations.

use anyhow::Result;
use std::collections::HashMap;

use crate::resolve;
use crate::util;

pub fn run(limit: usize, mailbox: Option<&str>) -> Result<()> {
    let conv_dir = match mailbox {
        Some(name) => resolve::mailbox_dir(name).join("conversations"),
        None => resolve::conversations_dir(),
    };

    if !conv_dir.is_dir() {
        println!("No conversations directory found at {}", conv_dir.display());
        return Ok(());
    }

    let histogram = build_subject_histogram(&conv_dir)?;
    if histogram.is_empty() {
        println!("No conversations found.");
        return Ok(());
    }

    // Sort by frequency descending, then alphabetically
    let mut entries: Vec<(&String, &usize)> = histogram.iter().collect();
    entries.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    let limit = limit.min(entries.len());
    println!("Top {} subject clusters:", limit);
    for (subject, count) in entries.iter().take(limit) {
        println!("  {:>3}x  {}", count, subject);
    }

    Ok(())
}

/// Build a frequency histogram of normalized subjects from conversation files.
fn build_subject_histogram(conv_dir: &std::path::Path) -> Result<HashMap<String, usize>> {
    let mut histogram: HashMap<String, usize> = HashMap::new();

    for entry in std::fs::read_dir(conv_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let slug = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if slug.is_empty() {
            continue;
        }
        let key = util::thread_key_from_subject(slug);
        if key.is_empty() {
            continue;
        }
        *histogram.entry(key).or_insert(0) += 1;
    }

    Ok(histogram)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_from_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello-world.md"), "# Hello").unwrap();
        std::fs::write(dir.path().join("hello-world-2.md"), "# Hello 2").unwrap();
        std::fs::write(dir.path().join("other-topic.md"), "# Other").unwrap();

        let hist = build_subject_histogram(dir.path()).unwrap();
        // "hello-world" and "hello-world-2" normalize differently
        assert!(hist.len() >= 2);
        assert!(hist.values().sum::<usize>() == 3);
    }

    #[test]
    fn histogram_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let hist = build_subject_histogram(dir.path()).unwrap();
        assert!(hist.is_empty());
    }

    #[test]
    fn histogram_skips_non_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "not markdown").unwrap();
        std::fs::write(dir.path().join("file.json"), "{}").unwrap();
        let hist = build_subject_histogram(dir.path()).unwrap();
        assert!(hist.is_empty());
    }
}
