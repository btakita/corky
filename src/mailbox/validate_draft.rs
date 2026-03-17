//! Validate draft markdown files.

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::draft;

static META_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\*\*(.+?)\*\*:\s*(.+)$").unwrap());

const REQUIRED_FIELDS: &[&str] = &["To"];
const RECOMMENDED_FIELDS: &[&str] = &["Status", "Author"];
const VALID_STATUSES: &[&str] = &["draft", "review", "approved", "sent", "scheduled"];

/// Validate a draft file. Returns list of issues (empty = valid).
pub fn validate_draft(path: &Path) -> Vec<String> {
    if !path.exists() {
        return vec![format!("File not found: {}", path.display())];
    }

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return vec![format!("Cannot read {}: {}", path.display(), e)],
    };

    if draft::is_yaml_format(&text) {
        return validate_yaml_draft(&text);
    }

    validate_legacy_draft(&text)
}

/// Validate a YAML frontmatter draft.
fn validate_yaml_draft(text: &str) -> Vec<String> {
    let mut issues = Vec::new();

    // Try to parse the YAML frontmatter
    let meta = match draft::parse_draft_yaml(text) {
        Some(m) => m,
        None => {
            issues.push("Invalid YAML frontmatter: could not parse".to_string());
            return issues;
        }
    };

    // Extract body (everything after the closing ---)
    let after_first = &text[4..]; // skip "---\n"
    let body_section = if let Some(end) = after_first.find("\n---") {
        let body_start = end + 4;
        if body_start < after_first.len() {
            after_first[body_start..].trim().to_string()
        } else {
            String::new()
        }
    } else {
        issues.push("Missing closing YAML frontmatter delimiter `---`".to_string());
        return issues;
    };

    // Check for subject heading in body
    let has_subject = body_section.lines().any(|line| line.starts_with("# "));
    if !has_subject {
        issues.push("Missing subject: no '# Subject' heading found in body".to_string());
    }

    // Required: to
    if meta.to.is_empty() {
        issues.push("Missing required field: to".to_string());
    }

    // Recommended: status, author
    if meta.author.is_none() {
        issues.push("Warning: missing recommended field: author".to_string());
    }

    // Status validation
    let status = meta.status.to_lowercase();
    if !status.is_empty() && !VALID_STATUSES.contains(&status.as_str()) {
        issues.push(format!(
            "Invalid status '{}'. Valid: {}",
            meta.status,
            VALID_STATUSES.join(", ")
        ));
    }

    if status == "draft" {
        issues.push(
            "Warning: Status is 'draft'. Set to 'review' when ready for review".to_string(),
        );
    }

    // Check body exists
    if body_section.is_empty() || body_section.lines().all(|l| l.starts_with("# ") || l.trim().is_empty()) {
        issues.push("Warning: empty body after subject heading".to_string());
    }

    issues
}

/// Validate a legacy `**Key**: value` format draft.
fn validate_legacy_draft(text: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();

    // Check for subject heading
    let has_subject = lines.iter().any(|line| line.starts_with("# "));
    if !has_subject {
        issues.push("Missing subject: no '# Subject' heading found".to_string());
    }

    // Parse metadata fields
    let mut meta: HashMap<String, String> = HashMap::new();
    for cap in META_RE.captures_iter(text) {
        meta.insert(cap[1].to_string(), cap[2].trim().to_string());
    }

    // Required fields
    for field in REQUIRED_FIELDS {
        if !meta.contains_key(*field) {
            issues.push(format!("Missing required field: **{}**", field));
        }
    }

    // Recommended fields (warn, don't error)
    for field in RECOMMENDED_FIELDS {
        if !meta.contains_key(*field) {
            issues.push(format!(
                "Warning: missing recommended field: **{}**",
                field
            ));
        }
    }

    // Status validation
    let status = meta
        .get("Status")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    if !status.is_empty() && !VALID_STATUSES.contains(&status.as_str()) {
        issues.push(format!(
            "Invalid status '{}'. Valid: {}",
            meta.get("Status").unwrap(),
            VALID_STATUSES.join(", ")
        ));
    }

    if status == "draft" {
        issues.push(
            "Warning: Status is 'draft'. Set to 'review' when ready for review".to_string(),
        );
    }

    // Check for --- separator
    let has_separator = lines.iter().any(|line| line.trim() == "---");
    if !has_separator {
        issues.push("Missing '---' separator between metadata and body".to_string());
    }

    // Check body exists after separator
    if has_separator
        && let Some(sep_idx) = lines.iter().position(|line| line.trim() == "---") {
            let body: String = lines[sep_idx + 1..].join("\n");
            if body.trim().is_empty() {
                issues.push("Warning: empty body after --- separator".to_string());
            }
        }

    issues
}

/// Resolve drafts directories based on scope, same pattern as find_unanswered::resolve_dirs().
fn resolve_draft_dirs(scope: &super::find_unanswered::Scope) -> Result<Vec<(String, PathBuf)>> {
    use super::find_unanswered::Scope;
    use crate::resolve;

    let data = resolve::data_dir();
    let root_drafts = data.join("drafts");
    let mailboxes_base = resolve::mailboxes_base_dir();

    let mut dirs = Vec::new();

    match scope {
        Scope::RootOnly => {
            dirs.push(("Root".to_string(), root_drafts));
        }
        Scope::Mailbox(name) => {
            let mb_drafts = mailboxes_base.join(name).join("drafts");
            if !mb_drafts.is_dir() {
                anyhow::bail!("Mailbox '{}' not found at {}", name, mb_drafts.display());
            }
            dirs.push((name.clone(), mb_drafts));
        }
        Scope::All => {
            if root_drafts.is_dir() {
                dirs.push(("Root".to_string(), root_drafts));
            }
            if mailboxes_base.is_dir() {
                let mut entries: Vec<_> = std::fs::read_dir(&mailboxes_base)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .collect();
                entries.sort_by_key(|e| e.file_name());
                for entry in entries {
                    let drafts = entry.path().join("drafts");
                    if drafts.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        dirs.push((name, drafts));
                    }
                }
            }
        }
    }

    Ok(dirs)
}

/// Collect all .md files under a directory.
fn collect_draft_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_draft_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

/// corky draft validate [ARGS...] — scope-based or file-based validation.
pub fn run_scoped(args: &[String]) -> Result<()> {
    use super::find_unanswered::Scope;
    use crate::resolve;

    // If args look like file paths (any contains '/' or '.' extension), treat as files
    let as_files = !args.is_empty()
        && args.iter().all(|a| {
            a != "." && (a.contains('/') || a.contains('.') || PathBuf::from(a).exists())
        });

    if as_files {
        let files: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        return run(&files);
    }

    // Otherwise parse as scope
    let scope = if args.is_empty() {
        Scope::All
    } else if args.len() == 1 && args[0] == "." {
        Scope::RootOnly
    } else if args.len() == 1 {
        // Check if it's a mailbox name
        let mailboxes_base = resolve::mailboxes_base_dir();
        if mailboxes_base.join(&args[0]).join("drafts").is_dir() {
            Scope::Mailbox(args[0].clone())
        } else {
            // Treat as a file path
            let files: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
            return run(&files);
        }
    } else {
        // Multiple args that don't look like files — treat as files anyway
        let files: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        return run(&files);
    };

    let dirs = resolve_draft_dirs(&scope)?;

    if dirs.is_empty() {
        println!("No drafts directories found.");
        return Ok(());
    }

    let mut all_files = Vec::new();
    for (_label, dir) in &dirs {
        collect_draft_files(dir, &mut all_files)?;
    }

    if all_files.is_empty() {
        println!("No draft files found.");
        return Ok(());
    }

    all_files.sort();
    run(&all_files)
}

/// corky validate-draft FILE [FILE...]
pub fn run(files: &[PathBuf]) -> Result<()> {
    let mut all_ok = true;

    for path in files {
        let issues = validate_draft(path);
        if !issues.is_empty() {
            all_ok = false;
            let errors: Vec<_> = issues
                .iter()
                .filter(|i| !i.starts_with("Warning:"))
                .collect();
            let warnings: Vec<_> = issues.iter().filter(|i| i.starts_with("Warning:")).collect();
            println!("{}:", path.display());
            for issue in errors {
                println!("  ERROR: {}", issue);
            }
            for issue in warnings {
                println!("  {}", issue);
            }
            println!();
        } else {
            println!("{}: OK", path.display());
        }
    }

    if !all_ok {
        std::process::exit(1);
    }
    Ok(())
}
