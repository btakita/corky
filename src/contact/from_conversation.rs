//! Create a contact from a conversation: extract participants, build enriched AGENTS.md.

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;

use super::add;
use crate::config::corky_config;
use crate::resolve;
use crate::sync::markdown::parse_thread_markdown;
use crate::util::slugify;

static EMAIL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<([^>]+)>").unwrap());

/// A participant extracted from conversation headers.
#[derive(Debug, Clone)]
struct Participant {
    display_name: String,
    emails: Vec<String>,
}

/// Create a contact from a conversation slug.
///
/// Algorithm:
/// 1. Find conversation file — search conversations/{slug}.md, then
///    mailboxes/*/conversations/{slug}.md.
/// 2. Parse thread via parse_thread_markdown().
/// 3. Load config for owner account emails.
/// 4. Extract non-owner participants from from, to, cc fields.
/// 5. Handle participant count: 0 = bail, 1 = auto-derive, 2+ = require name.
/// 6. Build Contact { emails }.
/// 7. Collect other participants for AGENTS.md template.
/// 8. Generate enriched AGENTS.md with topics, shared participants, email domain.
/// 9. Delegate to add::run_with_agents_md().
pub fn run(slug: &str, name: Option<&str>) -> Result<()> {
    // 1. Find conversation file
    let file_path = find_conversation(slug)?;

    // 2. Parse thread
    let text = std::fs::read_to_string(&file_path)?;
    let thread = parse_thread_markdown(&text)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse conversation: {}", file_path.display()))?;

    // 3. Load owner emails from config
    let owner_emails = load_owner_emails();

    // 4. Extract non-owner participants
    let participants = extract_participants(&thread, &owner_emails);

    if participants.is_empty() {
        anyhow::bail!("No non-owner participants found in this conversation");
    }

    // 5. Handle participant count
    let (selected, others) = select_participant(&participants, name)?;
    let contact_name = if let Some(n) = name {
        n.to_string()
    } else {
        slugify(&selected.display_name)
    };

    // 6. Build contact emails
    let emails = selected.emails.clone();

    // 7. Other participants (for AGENTS.md)
    let other_names: Vec<String> = others
        .iter()
        .map(|p| slugify(&p.display_name))
        .collect();

    // 8. Generate enriched AGENTS.md
    let topics = vec![thread.subject.clone()];
    let email_domain = selected
        .emails
        .first()
        .and_then(|e| e.split('@').nth(1))
        .map(|d| d.to_string());
    let agents_md = add::enriched_agents_md(
        &contact_name,
        &topics,
        &other_names,
        email_domain.as_deref(),
    );

    // 9. Delegate to shared creation logic
    println!("Creating contact '{}' from conversation '{}'", contact_name, slug);
    for email in &emails {
        println!("  Email: {}", email);
    }
    add::run_with_agents_md(&contact_name, &emails, &agents_md)
}

/// Find a conversation file by slug, searching root then mailboxes.
fn find_conversation(slug: &str) -> Result<std::path::PathBuf> {
    let data_dir = resolve::data_dir();
    let filename = format!("{}.md", slug);

    // Search root conversations/
    let root_path = data_dir.join("conversations").join(&filename);
    if root_path.exists() {
        return Ok(root_path);
    }

    // Search mailboxes/*/conversations/
    let mailboxes_dir = data_dir.join("mailboxes");
    if mailboxes_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&mailboxes_dir) {
            for entry in entries.flatten() {
                let mb_path = entry.path().join("conversations").join(&filename);
                if mb_path.exists() {
                    return Ok(mb_path);
                }
            }
        }

    anyhow::bail!(
        "Conversation '{}' not found.\nSearched:\n  - {}\n  - {}",
        slug,
        data_dir.join("conversations").display(),
        mailboxes_dir.join("*/conversations").display()
    )
}

/// Load owner email addresses from .corky.toml accounts.
fn load_owner_emails() -> Vec<String> {
    let config = match corky_config::try_load_config(None) {
        Some(c) => c,
        None => return Vec::new(),
    };
    config
        .accounts
        .values()
        .map(|a| a.user.to_lowercase())
        .collect()
}

/// Extract non-owner participants from all messages in the thread.
fn extract_participants(
    thread: &crate::sync::types::Thread,
    owner_emails: &[String],
) -> Vec<Participant> {
    let mut seen: std::collections::BTreeMap<String, Participant> = std::collections::BTreeMap::new();

    for msg in &thread.messages {
        for field in [&msg.from, &msg.to, &msg.cc] {
            extract_from_field(field, owner_emails, &mut seen);
        }
    }

    seen.into_values().collect()
}

/// Extract participants from a single header field (From, To, or CC).
fn extract_from_field(
    field: &str,
    owner_emails: &[String],
    seen: &mut std::collections::BTreeMap<String, Participant>,
) {
    for cap in EMAIL_RE.captures_iter(field) {
        let email = cap[1].to_lowercase();

        // Skip owner emails
        if owner_emails.iter().any(|o| o == &email) {
            continue;
        }

        // Extract display name
        let display_name = extract_display_name(field, &cap[1]);

        // Group by email — if same email seen with different display name, keep first
        let key = email.clone();
        let entry = seen.entry(key);
        match entry {
            std::collections::btree_map::Entry::Occupied(mut e) => {
                // Add email if not already present (handles multiple emails per person)
                if !e.get().emails.contains(&email) {
                    e.get_mut().emails.push(email);
                }
            }
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(Participant {
                    display_name,
                    emails: vec![email],
                });
            }
        }
    }
}

/// Extract display name from a "Name <email>" string.
fn extract_display_name(field: &str, email: &str) -> String {
    // Try to find "Name <email>" pattern near this email
    let pattern = format!("<{}>", email);
    if let Some(pos) = field.find(&pattern) {
        let before = &field[..pos].trim();
        // Walk back to find the start (after a comma or start of string)
        let start = before.rfind(',').map(|p| p + 1).unwrap_or(0);
        let name = before[start..].trim().trim_matches('"');
        if !name.is_empty() {
            return name.to_string();
        }
    }
    // Fallback: use email local part
    email.split('@').next().unwrap_or(email).to_string()
}

/// Select a participant based on count and optional name override.
fn select_participant<'a>(
    participants: &'a [Participant],
    name: Option<&str>,
) -> Result<(&'a Participant, Vec<&'a Participant>)> {
    if participants.len() == 1 {
        return Ok((&participants[0], Vec::new()));
    }

    // Multiple participants — need name to select
    if let Some(name) = name {
        let name_lower = name.to_lowercase();
        let slug_name = slugify(name);
        for (i, p) in participants.iter().enumerate() {
            let p_slug = slugify(&p.display_name);
            if p_slug == slug_name
                || p.display_name.to_lowercase() == name_lower
                || p.emails.iter().any(|e| e.starts_with(&format!("{}@", name_lower)))
            {
                let others: Vec<&Participant> = participants
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, p)| p)
                    .collect();
                return Ok((p, others));
            }
        }
        anyhow::bail!(
            "No participant matching '{}' found. Candidates:\n{}",
            name,
            format_candidates(participants)
        );
    }

    anyhow::bail!(
        "Multiple participants found. Specify name to select one:\n{}",
        format_candidates(participants)
    );
}

fn format_candidates(participants: &[Participant]) -> String {
    participants
        .iter()
        .map(|p| {
            format!(
                "  {} ({})",
                p.display_name,
                p.emails.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
