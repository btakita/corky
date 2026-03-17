//! Display aggregated contact info: config, AGENTS.md, matching threads.

use anyhow::Result;

use crate::config::contact;
use crate::resolve;

/// Show contact info and thread history.
///
/// Algorithm:
/// 1. Load contact from config — bail if not found.
/// 2. Print config section (emails).
/// 3. Print AGENTS.md if it exists.
/// 4. Scan manifest.toml files for matching threads.
/// 5. Print thread list sorted by last_updated descending.
/// 6. Print summary (thread count, last activity).
pub fn run(name: &str) -> Result<()> {
    // 1. Load contact
    let contacts = contact::load_contacts(None)?;
    let contact = contacts
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Contact '{}' not found in .corky.toml", name))?;

    // 2. Print config
    println!("Contact: {}", name);
    println!();
    if contact.emails.is_empty() {
        println!("  Emails: (none)");
    } else {
        println!("  Emails: {}", contact.emails.join(", "));
    }
    println!();

    // 3. Print AGENTS.md
    let agents_path = resolve::contacts_dir().join(name).join("AGENTS.md");
    if agents_path.exists() {
        let content = std::fs::read_to_string(&agents_path)?;
        println!("--- AGENTS.md ---");
        println!("{}", content.trim());
        println!();
    }

    // 4. Scan manifests for matching threads
    let data_dir = resolve::data_dir();
    let mut all_threads: Vec<(String, String, String, String)> = Vec::new(); // (scope, date, slug, subject)

    // Root manifest
    let root_manifest = data_dir.join("manifest.toml");
    if root_manifest.exists() {
        collect_threads_from_manifest(&root_manifest, name, "root", &mut all_threads)?;
    }

    // Mailbox manifests
    let mailboxes_dir = data_dir.join("mailboxes");
    if mailboxes_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&mailboxes_dir) {
            let mut mb_entries: Vec<_> = entries.flatten().collect();
            mb_entries.sort_by_key(|e| e.file_name());
            for entry in mb_entries {
                let mb_name = entry.file_name().to_string_lossy().to_string();
                let mb_manifest = entry.path().join("manifest.toml");
                if mb_manifest.exists() {
                    collect_threads_from_manifest(&mb_manifest, name, &mb_name, &mut all_threads)?;
                }
            }
        }

    // 5. Print thread list
    if all_threads.is_empty() {
        println!("--- Threads ---");
        println!("  No matching threads found in manifest.");
    } else {
        // Sort by date descending
        all_threads.sort_by(|a, b| b.1.cmp(&a.1));

        println!("--- Threads ({}) ---", all_threads.len());
        for (scope, date, slug, subject) in &all_threads {
            // Truncate date to just the date portion for display
            let short_date = date.split(' ').take(4).collect::<Vec<_>>().join(" ");
            let scope_prefix = if scope == "root" {
                String::new()
            } else {
                format!("[{}] ", scope)
            };
            println!("  {}  {:<30} {}{}", short_date, slug, scope_prefix, subject);
        }
    }
    println!();

    // 6. Summary
    if !all_threads.is_empty() {
        println!(
            "Last activity: {}",
            all_threads.first().map(|t| t.1.as_str()).unwrap_or("unknown")
        );
    }

    Ok(())
}

/// Collect threads mentioning a contact from a manifest.toml file.
fn collect_threads_from_manifest(
    manifest_path: &std::path::Path,
    contact_name: &str,
    scope: &str,
    out: &mut Vec<(String, String, String, String)>,
) -> Result<()> {
    let content = std::fs::read_to_string(manifest_path)?;
    let manifest: toml::Value = toml::from_str(&content)?;

    let threads = match manifest.get("threads").and_then(|t| t.as_table()) {
        Some(t) => t,
        None => return Ok(()),
    };

    for (slug, data) in threads {
        let contacts = data
            .get("contacts")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if contacts.contains(&contact_name) {
            let subject = data
                .get("subject")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let last_updated = data
                .get("last_updated")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            out.push((
                scope.to_string(),
                last_updated,
                slug.clone(),
                subject,
            ));
        }
    }

    Ok(())
}
