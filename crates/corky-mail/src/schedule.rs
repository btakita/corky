//! Unified scheduling for social and email drafts.
//!
//! Scans social/ and drafts/ (including mailboxes/*/drafts/) for items with
//! a scheduled_at time in the past, then dispatches to existing publish functions.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

use crate::resolve;
use crate::social::draft::SocialDraft;

/// Grace window: items scheduled up to this many seconds in the future
/// are still considered due (handles clock skew / cron drift).
const GRACE_SECONDS: i64 = 30;

/// The kind of scheduled item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledKind {
    Social,
    Email,
}

impl std::fmt::Display for ScheduledKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduledKind::Social => write!(f, "social"),
            ScheduledKind::Email => write!(f, "email"),
        }
    }
}

/// A pending scheduled item discovered by scanning the filesystem.
#[derive(Debug, Clone)]
pub struct ScheduledItem {
    pub path: PathBuf,
    pub kind: ScheduledKind,
    pub scheduled_at: DateTime<Utc>,
    /// Human-readable label (platform or subject).
    pub label: String,
}

/// Result of processing one scheduled item.
#[derive(Debug)]
pub struct ProcessResult {
    pub path: PathBuf,
    pub kind: ScheduledKind,
    pub success: bool,
    pub message: String,
}

/// Scan all draft directories for scheduled items.
pub fn scan_scheduled(now: DateTime<Utc>) -> Result<Vec<ScheduledItem>> {
    let deadline = now + chrono::Duration::seconds(GRACE_SECONDS);
    let mut items = Vec::new();

    // Scan social drafts
    let social_dir = resolve::social_dir();
    if social_dir.is_dir() {
        scan_social_dir(&social_dir, deadline, &mut items)?;
    }

    // Scan root drafts/
    let drafts_dir = resolve::drafts_dir();
    if drafts_dir.is_dir() {
        scan_email_dir(&drafts_dir, deadline, &mut items)?;
    }

    // Scan mailboxes/*/drafts/
    let mb_base = resolve::mailboxes_base_dir();
    if mb_base.is_dir()
        && let Ok(entries) = std::fs::read_dir(&mb_base)
    {
        for entry in entries.flatten() {
            let mb_drafts = entry.path().join("drafts");
            if mb_drafts.is_dir() {
                scan_email_dir(&mb_drafts, deadline, &mut items)?;
            }
        }
    }

    // Sort by scheduled_at ascending (earliest first)
    items.sort_by_key(|item| item.scheduled_at);
    Ok(items)
}

/// Scan a social/ directory for drafts with scheduled_at <= deadline.
/// Accepts `draft` or `ready` status — setting `scheduled_at` implies readiness.
/// Skips `published` items (already posted).
fn scan_social_dir(
    dir: &Path,
    deadline: DateTime<Utc>,
    items: &mut Vec<ScheduledItem>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false)
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(draft) = SocialDraft::parse(&content)
        {
            // Skip terminal items (published/failed) — prevents double-publish and
            // stops the scheduler from re-running a draft whose publish failed.
            if draft.meta.status.is_terminal() {
                continue;
            }
            if let Some(scheduled_at) = draft.meta.scheduled_at
                && scheduled_at <= deadline
            {
                items.push(ScheduledItem {
                    path,
                    kind: ScheduledKind::Social,
                    scheduled_at,
                    label: format!("{}", draft.meta.platform),
                });
            }
        }
    }
    Ok(())
}

/// Scan an email drafts/ directory for scheduled drafts with Scheduled-At <= deadline.
fn scan_email_dir(
    dir: &Path,
    deadline: DateTime<Utc>,
    items: &mut Vec<ScheduledItem>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false)
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Some(item) = parse_email_scheduled(&path, &content, deadline)
        {
            items.push(item);
        }
    }
    Ok(())
}

/// Try to extract a ScheduledItem from an email draft's content.
fn parse_email_scheduled(
    path: &Path,
    content: &str,
    deadline: DateTime<Utc>,
) -> Option<ScheduledItem> {
    // Try YAML frontmatter first
    if let Some(meta) = crate::draft::parse_draft_yaml(content) {
        if !meta.status.is_scheduled() {
            return None;
        }
        let scheduled_at = meta.scheduled_at?;
        if scheduled_at > deadline {
            return None;
        }

        // Extract subject from body (first # heading after frontmatter)
        let subject = extract_subject_from_content(content);

        return Some(ScheduledItem {
            path: path.to_path_buf(),
            kind: ScheduledKind::Email,
            scheduled_at,
            label: if subject.is_empty() {
                "email".to_string()
            } else {
                subject
            },
        });
    }

    // Fall back to legacy regex parsing
    let mut status = None;
    let mut scheduled_at_str = None;
    let mut subject = String::new();

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            subject = rest.trim().to_string();
        }
        if let Some(rest) = line.strip_prefix("**Status**: ") {
            status = Some(rest.trim().to_string());
        }
        if let Some(rest) = line.strip_prefix("**Scheduled-At**: ") {
            scheduled_at_str = Some(rest.trim().to_string());
        }
    }

    let status = status?;
    if status
        .parse::<crate::draft::EmailDraftStatus>()
        .map(|s| !s.is_scheduled())
        .unwrap_or(true)
    {
        return None;
    }

    let scheduled_at_str = scheduled_at_str?;
    let scheduled_at: DateTime<Utc> = scheduled_at_str.parse().ok()?;

    if scheduled_at > deadline {
        return None;
    }

    Some(ScheduledItem {
        path: path.to_path_buf(),
        kind: ScheduledKind::Email,
        scheduled_at,
        label: if subject.is_empty() {
            "email".to_string()
        } else {
            subject
        },
    })
}

/// Extract the subject from draft content (first `# Heading` line).
fn extract_subject_from_content(content: &str) -> String {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            return rest.trim().to_string();
        }
    }
    String::new()
}

/// Run the scheduler: find all due items and publish them.
pub fn run(dry_run: bool) -> Result<()> {
    let now = Utc::now();
    let items = scan_scheduled(now)?;

    if items.is_empty() {
        if dry_run {
            println!("No scheduled items due.");
        }
        return Ok(());
    }

    let mut results = Vec::new();
    for item in &items {
        if dry_run {
            println!(
                "[dry-run] Would publish {} {} (scheduled {}): {}",
                item.kind,
                item.path.display(),
                item.scheduled_at.format("%Y-%m-%d %H:%M UTC"),
                item.label,
            );
            continue;
        }

        let result = match item.kind {
            ScheduledKind::Social => match crate::social::publish::publish(&item.path, false) {
                Ok(()) => ProcessResult {
                    path: item.path.clone(),
                    kind: item.kind,
                    success: true,
                    message: format!("Published {}", item.label),
                },
                Err(e) => ProcessResult {
                    path: item.path.clone(),
                    kind: item.kind,
                    success: false,
                    message: format!("Failed: {}", e),
                },
            },
            ScheduledKind::Email => match crate::draft::run(&item.path, true) {
                Ok(()) => ProcessResult {
                    path: item.path.clone(),
                    kind: item.kind,
                    success: true,
                    message: format!("Sent {}", item.label),
                },
                Err(e) => ProcessResult {
                    path: item.path.clone(),
                    kind: item.kind,
                    success: false,
                    message: format!("Failed: {}", e),
                },
            },
        };
        results.push(result);
    }

    if dry_run {
        return Ok(());
    }

    // Report results
    let mut errors = 0;
    for result in &results {
        if result.success {
            println!(
                "[ok] {} {}: {}",
                result.kind,
                result.path.display(),
                result.message
            );
        } else {
            eprintln!(
                "[error] {} {}: {}",
                result.kind,
                result.path.display(),
                result.message
            );
            errors += 1;
        }
    }

    if errors > 0 {
        bail!("{} of {} scheduled item(s) failed", errors, results.len());
    }

    Ok(())
}

/// List all pending scheduled items (due and future).
pub fn list() -> Result<()> {
    let now = Utc::now();
    // Use a far-future deadline to find all scheduled items (not just due ones)
    let far_future = now + chrono::Duration::days(365 * 10);

    let mut items = Vec::new();

    // Scan social drafts
    let social_dir = resolve::social_dir();
    if social_dir.is_dir() {
        scan_social_dir(&social_dir, far_future, &mut items)?;
    }

    // Scan root drafts/
    let drafts_dir = resolve::drafts_dir();
    if drafts_dir.is_dir() {
        scan_email_dir(&drafts_dir, far_future, &mut items)?;
    }

    // Scan mailboxes/*/drafts/
    let mb_base = resolve::mailboxes_base_dir();
    if mb_base.is_dir()
        && let Ok(entries) = std::fs::read_dir(&mb_base)
    {
        for entry in entries.flatten() {
            let mb_drafts = entry.path().join("drafts");
            if mb_drafts.is_dir() {
                scan_email_dir(&mb_drafts, far_future, &mut items)?;
            }
        }
    }

    items.sort_by_key(|item| item.scheduled_at);

    if items.is_empty() {
        println!("No scheduled items.");
        return Ok(());
    }

    for item in &items {
        let due = if item.scheduled_at <= now {
            " [DUE]"
        } else {
            ""
        };
        println!(
            "  {} {} — {} ({}){due}",
            item.scheduled_at.format("%Y-%m-%d %H:%M UTC"),
            item.kind,
            item.label,
            item.path.display(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::TempDir;

    fn make_social_draft(scheduled_at: Option<DateTime<Utc>>, status: &str) -> String {
        let sched = match scheduled_at {
            Some(t) => format!("scheduled_at: \"{}\"", t.to_rfc3339()),
            None => "scheduled_at: null".to_string(),
        };
        format!(
            "---\nplatform: linkedin\nauthor: test\nvisibility: public\nstatus: {status}\ntags: []\n{sched}\n---\nTest post\n"
        )
    }

    fn make_email_draft(scheduled_at: Option<DateTime<Utc>>, status: &str) -> String {
        let mut lines = vec![
            "# Test Subject".to_string(),
            String::new(),
            "**To**: test@example.com".to_string(),
            format!("**Status**: {status}"),
        ];
        if let Some(t) = scheduled_at {
            lines.push(format!("**Scheduled-At**: {}", t.to_rfc3339()));
        }
        lines.push(String::new());
        lines.push("---".to_string());
        lines.push(String::new());
        lines.push("Body text".to_string());
        lines.join("\n")
    }

    // S1: No scheduled items
    #[test]
    fn s1_no_scheduled_items() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();
        let drafts = tmp.path().join("drafts");
        std::fs::create_dir_all(&drafts).unwrap();

        // Scan directly with the low-level functions
        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        scan_email_dir(&drafts, deadline, &mut items).unwrap();
        assert!(items.is_empty());
    }

    // S2: Item due — social draft with scheduled_at in the past
    #[test]
    fn s2_social_item_due() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();

        let past = Utc::now() - Duration::minutes(5);
        let content = make_social_draft(Some(past), "ready");
        std::fs::write(social.join("test-post.md"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ScheduledKind::Social);
    }

    // S3: Item due — email draft with scheduled_at in the past
    #[test]
    fn s3_email_item_due() {
        let tmp = TempDir::new().unwrap();
        let drafts = tmp.path().join("drafts");
        std::fs::create_dir_all(&drafts).unwrap();

        let past = Utc::now() - Duration::minutes(5);
        let content = make_email_draft(Some(past), "scheduled");
        std::fs::write(drafts.join("test-email.md"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_email_dir(&drafts, deadline, &mut items).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ScheduledKind::Email);
        assert_eq!(items[0].label, "Test Subject");
    }

    // S4: Item in future — not due
    #[test]
    fn s4_item_in_future() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();

        let future = Utc::now() + Duration::hours(1);
        let content = make_social_draft(Some(future), "ready");
        std::fs::write(social.join("future.md"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        assert!(items.is_empty());
    }

    // S5: scheduled_at missing on ready item — skipped
    #[test]
    fn s5_no_scheduled_at() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();

        let content = make_social_draft(None, "ready");
        std::fs::write(social.join("no-schedule.md"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        assert!(items.is_empty());
    }

    // S6: Multiple items due — sorted by scheduled_at
    #[test]
    fn s6_multiple_items_sorted() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();

        let earlier = Utc::now() - Duration::hours(2);
        let later = Utc::now() - Duration::minutes(5);

        std::fs::write(
            social.join("later.md"),
            make_social_draft(Some(later), "ready"),
        )
        .unwrap();
        std::fs::write(
            social.join("earlier.md"),
            make_social_draft(Some(earlier), "ready"),
        )
        .unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        items.sort_by_key(|i| i.scheduled_at);

        assert_eq!(items.len(), 2);
        assert!(items[0].scheduled_at < items[1].scheduled_at);
    }

    // S7: Email draft with wrong status — skipped
    #[test]
    fn s7_email_wrong_status() {
        let tmp = TempDir::new().unwrap();
        let drafts = tmp.path().join("drafts");
        std::fs::create_dir_all(&drafts).unwrap();

        let past = Utc::now() - Duration::minutes(5);
        let content = make_email_draft(Some(past), "draft");
        std::fs::write(drafts.join("not-scheduled.md"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_email_dir(&drafts, deadline, &mut items).unwrap();
        assert!(items.is_empty());
    }

    // S8: Social draft with draft status + scheduled_at — picked up (scheduling implies readiness)
    #[test]
    fn s8_social_draft_with_schedule() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();

        let past = Utc::now() - Duration::minutes(5);
        let content = make_social_draft(Some(past), "draft");
        std::fs::write(social.join("scheduled-draft.md"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        assert_eq!(items.len(), 1);
    }

    // S8b: Social draft with published status — skipped (prevents double-publish)
    #[test]
    fn s8b_social_published_skipped() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();

        let past = Utc::now() - Duration::minutes(5);
        let content = make_social_draft(Some(past), "published");
        std::fs::write(social.join("already-published.md"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        assert!(items.is_empty());
    }

    // S9: Non-md files ignored
    #[test]
    fn s9_non_md_ignored() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();

        let past = Utc::now() - Duration::minutes(5);
        let content = make_social_draft(Some(past), "ready");
        std::fs::write(social.join("readme.txt"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        assert!(items.is_empty());
    }

    // S10: Grace window — item slightly in future still counts
    #[test]
    fn s10_grace_window() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();

        // Scheduled 15 seconds in the future (within 30s grace)
        let near_future = Utc::now() + Duration::seconds(15);
        let content = make_social_draft(Some(near_future), "ready");
        std::fs::write(social.join("grace.md"), &content).unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        assert_eq!(items.len(), 1);
    }

    // Email scheduled_at parse round-trip
    #[test]
    fn email_scheduled_parse() {
        let now = Utc::now();
        let content = make_email_draft(Some(now - Duration::minutes(1)), "scheduled");
        let far_future = now + Duration::days(365);
        let item = parse_email_scheduled(Path::new("test.md"), &content, far_future);
        assert!(item.is_some());
        let item = item.unwrap();
        assert_eq!(item.kind, ScheduledKind::Email);
        assert_eq!(item.label, "Test Subject");
    }

    // Email with no subject falls back to "email"
    #[test]
    fn email_no_subject_fallback() {
        let now = Utc::now();
        let content = format!(
            "**To**: test@example.com\n**Status**: scheduled\n**Scheduled-At**: {}\n\n---\n\nBody",
            (now - Duration::minutes(1)).to_rfc3339()
        );
        let far_future = now + Duration::days(365);
        let item = parse_email_scheduled(Path::new("test.md"), &content, far_future);
        assert!(item.is_some());
        assert_eq!(item.unwrap().label, "email");
    }

    // YAML frontmatter email draft — scheduled and due
    #[test]
    fn yaml_email_scheduled_due() {
        let now = Utc::now();
        let past = now - Duration::minutes(5);
        let content = format!(
            "---\nto: test@example.com\nstatus: scheduled\nscheduled_at: \"{}\"\n---\n\n# YAML Subject\n\nBody\n",
            past.to_rfc3339()
        );
        let far_future = now + Duration::days(365);
        let item = parse_email_scheduled(Path::new("test.md"), &content, far_future);
        assert!(item.is_some());
        let item = item.unwrap();
        assert_eq!(item.kind, ScheduledKind::Email);
        assert_eq!(item.label, "YAML Subject");
    }

    // YAML frontmatter email draft — wrong status, should be skipped
    #[test]
    fn yaml_email_not_scheduled() {
        let now = Utc::now();
        let past = now - Duration::minutes(5);
        let content = format!(
            "---\nto: test@example.com\nstatus: draft\nscheduled_at: \"{}\"\n---\n\n# Subject\n\nBody\n",
            past.to_rfc3339()
        );
        let far_future = now + Duration::days(365);
        let item = parse_email_scheduled(Path::new("test.md"), &content, far_future);
        assert!(item.is_none());
    }

    // S11: scan_scheduled returns both social and email kinds from a single data dir
    #[test]
    fn s11_scan_both_kinds() {
        let tmp = TempDir::new().unwrap();
        let social = tmp.path().join("social");
        std::fs::create_dir_all(&social).unwrap();
        let drafts = tmp.path().join("drafts");
        std::fs::create_dir_all(&drafts).unwrap();

        let past = Utc::now() - Duration::minutes(5);
        std::fs::write(
            social.join("post.md"),
            make_social_draft(Some(past), "ready"),
        )
        .unwrap();
        std::fs::write(
            drafts.join("email.md"),
            make_email_draft(Some(past), "scheduled"),
        )
        .unwrap();

        let now = Utc::now();
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let mut items = Vec::new();
        scan_social_dir(&social, deadline, &mut items).unwrap();
        scan_email_dir(&drafts, deadline, &mut items).unwrap();
        items.sort_by_key(|i| i.scheduled_at);

        assert_eq!(items.len(), 2);
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&ScheduledKind::Social));
        assert!(kinds.contains(&ScheduledKind::Email));
    }

    // S12: ProcessResult structural test
    #[test]
    fn s12_process_result_fields() {
        let result = ProcessResult {
            path: PathBuf::from("social/test.md"),
            kind: ScheduledKind::Social,
            success: true,
            message: "Published linkedin".to_string(),
        };
        assert!(result.success);
        assert_eq!(result.kind, ScheduledKind::Social);
        assert!(result.message.contains("Published"));

        let failed = ProcessResult {
            path: PathBuf::from("drafts/email.md"),
            kind: ScheduledKind::Email,
            success: false,
            message: "Failed: SMTP error".to_string(),
        };
        assert!(!failed.success);
        assert_eq!(failed.kind, ScheduledKind::Email);
    }

    // S13: ScheduledKind Display trait
    #[test]
    fn s13_kind_display() {
        assert_eq!(format!("{}", ScheduledKind::Social), "social");
        assert_eq!(format!("{}", ScheduledKind::Email), "email");
    }

    // YAML frontmatter email draft — scheduled but in future, not due
    #[test]
    fn yaml_email_scheduled_future() {
        let now = Utc::now();
        let future = now + Duration::hours(2);
        let content = format!(
            "---\nto: test@example.com\nstatus: scheduled\nscheduled_at: \"{}\"\n---\n\n# Subject\n\nBody\n",
            future.to_rfc3339()
        );
        let deadline = now + Duration::seconds(GRACE_SECONDS);
        let item = parse_email_scheduled(Path::new("test.md"), &content, deadline);
        assert!(item.is_none());
    }
}
