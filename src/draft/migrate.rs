//! Migrate legacy `**Key**: value` email drafts to YAML frontmatter format.

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;

use super::EmailDraftMeta;
use crate::resolve;

static META_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\*\*(.+?)\*\*:\s*(.+)$").unwrap());

/// Migrate all legacy email drafts to YAML frontmatter format.
pub fn run(dry_run: bool) -> Result<()> {
    let mut dirs = Vec::new();

    // Root drafts/
    let drafts_dir = resolve::drafts_dir();
    if drafts_dir.is_dir() {
        dirs.push(drafts_dir);
    }

    // mailboxes/*/drafts/
    let mb_base = resolve::mailboxes_base_dir();
    if mb_base.is_dir()
        && let Ok(entries) = std::fs::read_dir(&mb_base) {
            for entry in entries.flatten() {
                let mb_drafts = entry.path().join("drafts");
                if mb_drafts.is_dir() {
                    dirs.push(mb_drafts);
                }
            }
        }

    if dirs.is_empty() {
        println!("No drafts directories found.");
        return Ok(());
    }

    let mut total = 0;
    let mut migrated = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for dir in &dirs {
        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            total += 1;

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("  [error] {}: {}", path.display(), e);
                    errors += 1;
                    continue;
                }
            };

            if super::is_yaml_format(&content) {
                skipped += 1;
                if dry_run {
                    println!("  [skip] {} (already YAML)", path.display());
                }
                continue;
            }

            match convert_legacy_to_yaml(&content) {
                Ok(new_content) => {
                    if dry_run {
                        println!("  [would migrate] {}", path.display());
                    } else {
                        if let Err(e) = std::fs::write(&path, &new_content) {
                            eprintln!("  [error] {}: {}", path.display(), e);
                            errors += 1;
                            continue;
                        }
                        println!("  [migrated] {}", path.display());
                    }
                    migrated += 1;
                }
                Err(e) => {
                    eprintln!("  [error] {}: {}", path.display(), e);
                    errors += 1;
                }
            }
        }
    }

    println!();
    if dry_run {
        println!(
            "Dry run: {} total, {} would migrate, {} already YAML, {} errors",
            total, migrated, skipped, errors
        );
    } else {
        println!(
            "Done: {} total, {} migrated, {} already YAML, {} errors",
            total, migrated, skipped, errors
        );
    }

    Ok(())
}

/// Convert a legacy `**Key**: value` draft to YAML frontmatter format.
fn convert_legacy_to_yaml(content: &str) -> Result<String> {
    let lines: Vec<&str> = content.split('\n').collect();

    // Extract subject from # heading
    let subject = lines
        .iter()
        .find_map(|line| line.strip_prefix("# ").map(|s| s.trim().to_string()))
        .unwrap_or_default();

    // Parse metadata fields
    let mut to = String::new();
    let mut cc: Option<String> = None;
    let mut status = String::from("draft");
    let mut author: Option<String> = None;
    let mut account: Option<String> = None;
    let mut from: Option<String> = None;
    let mut in_reply_to: Option<String> = None;
    let mut scheduled_at: Option<chrono::DateTime<chrono::Utc>> = None;

    for cap in META_RE.captures_iter(content) {
        let key = cap[1].to_string();
        let val = cap[2].trim().to_string();
        match key.as_str() {
            "To" => to = val,
            "CC" => cc = Some(val),
            "Status" => status = val.to_lowercase(),
            "Author" => author = Some(val),
            "Account" => account = Some(val),
            "From" => from = Some(val),
            "In-Reply-To" => in_reply_to = Some(val),
            "Scheduled-At" => {
                scheduled_at = val.parse().ok();
            }
            _ => {} // ignore unknown fields
        }
    }

    if to.is_empty() {
        anyhow::bail!("Missing **To** field");
    }

    let meta = EmailDraftMeta {
        to,
        subject: None, // Legacy format uses # heading in body
        cc,
        status,
        author,
        account,
        from,
        in_reply_to,
        scheduled_at,
        attachments: Vec::new(),
    };

    let yaml = serde_yaml::to_string(&meta)?;

    // Find body: everything after the first --- separator
    let body = if let Some(sep_idx) = lines.iter().position(|line| line.trim() == "---") {
        lines[sep_idx + 1..].join("\n").trim().to_string()
    } else {
        String::new()
    };

    let mut result = format!("---\n{}---\n\n# {}\n", yaml, subject);
    if !body.is_empty() {
        result.push('\n');
        result.push_str(&body);
        result.push('\n');
    } else {
        result.push('\n');
    }

    Ok(result)
}

/// Convert a single file's content (for use in tests and other modules).
pub fn convert_content(content: &str) -> Result<String> {
    convert_legacy_to_yaml(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_minimal() {
        let legacy = "# Hello\n\n**To**: alice@example.com\n**Status**: draft\n\n---\n\nBody text\n";
        let result = convert_legacy_to_yaml(legacy).unwrap();
        assert!(result.starts_with("---\n"));
        assert!(result.contains("to: alice@example.com"));
        assert!(result.contains("status: draft"));
        assert!(result.contains("# Hello"));
        assert!(result.contains("Body text"));
    }

    #[test]
    fn test_convert_all_fields() {
        let legacy = "\
# Test Subject

**To**: alice@example.com
**CC**: bob@example.com
**Status**: review
**Author**: Brian
**Account**: personal
**From**: brian@example.com
**In-Reply-To**: <msg-1>

---

Hello body
";
        let result = convert_legacy_to_yaml(legacy).unwrap();
        assert!(result.starts_with("---\n"));
        assert!(result.contains("to: alice@example.com"));
        assert!(result.contains("cc: bob@example.com"));
        assert!(result.contains("status: review"));
        assert!(result.contains("author: Brian"));
        assert!(result.contains("account: personal"));
        assert!(result.contains("from: brian@example.com"));
        assert!(result.contains("in_reply_to:"));
        assert!(result.contains("# Test Subject"));
        assert!(result.contains("Hello body"));
    }

    #[test]
    fn test_convert_missing_to_fails() {
        let legacy = "# Hello\n\n**Status**: draft\n\n---\n\nBody\n";
        assert!(convert_legacy_to_yaml(legacy).is_err());
    }

    #[test]
    fn test_convert_roundtrip() {
        let legacy = "# Subject\n\n**To**: a@b.com\n**Status**: draft\n**Author**: Alice\n\n---\n\nBody here\n";
        let yaml_content = convert_legacy_to_yaml(legacy).unwrap();

        // Should be parseable by the YAML parser
        assert!(super::super::is_yaml_format(&yaml_content));
        let meta = super::super::parse_draft_yaml(&yaml_content).unwrap();
        assert_eq!(meta.to, "a@b.com");
        assert_eq!(meta.status, "draft");
        assert_eq!(meta.author.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_migrate_filesystem() {
        let tmp = tempfile::TempDir::new().unwrap();
        let drafts = tmp.path().join("drafts");
        std::fs::create_dir_all(&drafts).unwrap();

        let legacy = "# Hello\n\n**To**: a@b.com\n**Status**: draft\n\n---\n\nBody\n";
        let yaml = "---\nto: a@b.com\nstatus: draft\n---\n\n# Already YAML\n\n";

        std::fs::write(drafts.join("legacy.md"), legacy).unwrap();
        std::fs::write(drafts.join("yaml.md"), yaml).unwrap();
        std::fs::write(drafts.join("readme.txt"), "not a draft").unwrap();

        // Read legacy and convert
        let content = std::fs::read_to_string(drafts.join("legacy.md")).unwrap();
        assert!(!super::super::is_yaml_format(&content));

        let converted = convert_legacy_to_yaml(&content).unwrap();
        assert!(super::super::is_yaml_format(&converted));

        // YAML file should be detected as already YAML
        let yaml_content = std::fs::read_to_string(drafts.join("yaml.md")).unwrap();
        assert!(super::super::is_yaml_format(&yaml_content));
    }
}
