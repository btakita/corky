//! Find threads where the last message is not from the owner.

use anyhow::{Result, bail};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::PathBuf;

use crate::resolve;

static SENDER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^## (.+?) \u{2014}").unwrap());
static DATE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*Last updated\*\*:\s*(\S+)").unwrap());
static LABELS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*Labels?\*\*:\s*(.+)").unwrap());

/// Scope for unanswered thread search.
pub enum Scope {
    /// Root conversations/ + all mailboxes/*/conversations/
    All,
    /// Just root conversations/
    RootOnly,
    /// A specific mailbox
    Mailbox(String),
}

impl Scope {
    pub fn from_arg(arg: Option<&str>) -> Self {
        match arg {
            None => Scope::All,
            Some(".") => Scope::RootOnly,
            Some(name) => Scope::Mailbox(name.to_string()),
        }
    }
}

fn last_sender(text: &str) -> String {
    SENDER_RE
        .captures_iter(text)
        .last()
        .map(|cap| cap[1].trim().to_string())
        .unwrap_or_default()
}

fn thread_date(text: &str) -> String {
    DATE_RE
        .captures(text)
        .map(|cap| cap[1].to_string())
        .unwrap_or_default()
}

fn thread_labels(text: &str) -> String {
    LABELS_RE
        .captures(text)
        .map(|cap| cap[1].trim().to_string())
        .unwrap_or_default()
}

/// Scan a conversations directory and return unanswered threads.
/// Each entry: (date, labels, filename, sender).
fn scan_dir(
    dir: &std::path::Path,
    from_lower: &str,
) -> Result<Vec<(String, String, String, String)>> {
    let mut results = Vec::new();
    if !dir.is_dir() {
        return Ok(results);
    }

    let mut md_files = Vec::new();
    collect_md_files(dir, &mut md_files)?;
    md_files.sort();

    for thread_file in &md_files {
        let text = std::fs::read_to_string(thread_file)?;
        let sender = last_sender(&text);
        if !sender.is_empty() && !sender.to_lowercase().contains(from_lower) {
            let labels = {
                let l = thread_labels(&text);
                if l.is_empty() {
                    thread_file
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                } else {
                    l
                }
            };
            let date = {
                let d = thread_date(&text);
                if d.is_empty() {
                    "unknown".to_string()
                } else {
                    d
                }
            };
            let filename = thread_file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            results.push((date, labels, filename, sender));
        }
    }

    Ok(results)
}

/// Build list of (group_label, conversations_dir) pairs based on scope.
fn resolve_dirs(scope: &Scope) -> Result<Vec<(String, PathBuf)>> {
    let data = resolve::data_dir();
    let root_convos = data.join("conversations");
    let mailboxes_base = resolve::mailboxes_base_dir();

    let mut dirs = Vec::new();

    match scope {
        Scope::RootOnly => {
            dirs.push(("Root".to_string(), root_convos));
        }
        Scope::Mailbox(name) => {
            let mb_convos = mailboxes_base.join(name).join("conversations");
            if !mb_convos.is_dir() {
                bail!("Mailbox '{}' not found at {}", name, mb_convos.display());
            }
            dirs.push((name.clone(), mb_convos));
        }
        Scope::All => {
            if root_convos.is_dir() {
                dirs.push(("Root".to_string(), root_convos));
            }
            if mailboxes_base.is_dir() {
                let mut entries: Vec<_> = std::fs::read_dir(&mailboxes_base)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .collect();
                entries.sort_by_key(|e| e.file_name());
                for entry in entries {
                    let convos = entry.path().join("conversations");
                    if convos.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        dirs.push((name, convos));
                    }
                }
            }
        }
    }

    Ok(dirs)
}

/// corky unanswered [SCOPE] [--from NAME]
pub fn run(scope: Scope, from_name: &str) -> Result<()> {
    let dirs = resolve_dirs(&scope)?;

    if dirs.is_empty() {
        eprintln!("No conversations directories found.");
        std::process::exit(1);
    }

    let from_lower = from_name.to_lowercase();
    let multi = dirs.len() > 1;

    let mut total = 0usize;

    for (label, dir) in &dirs {
        let mut unanswered = scan_dir(dir, &from_lower)?;
        if unanswered.is_empty() {
            continue;
        }
        // Sort by date descending (newest first)
        unanswered.sort_by(|a, b| b.0.cmp(&a.0));
        total += unanswered.len();

        if multi {
            println!("{} ({} unanswered):\n", label, unanswered.len());
        } else {
            println!("Unanswered threads ({}):\n", unanswered.len());
        }

        for (date, labels, filename, sender) in &unanswered {
            println!("  [{}] {}", labels, filename);
            println!("           Last from: {} ({})", sender, date);
            println!();
        }
    }

    if total == 0 {
        println!("No unanswered threads found.");
    }

    Ok(())
}

fn collect_md_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_md_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}
