//! IMAP email sync — fetch threads from IMAP and write to Markdown.

pub mod auth;
pub mod folders;
pub mod gmail_api_sync;
pub mod imap_sync;
pub mod imports;
pub mod manifest;
pub mod markdown;
pub mod markdown_clean;
pub mod routes;
pub mod slack_import;
pub mod sms_import;
pub mod telegram_import;
pub mod types;

use anyhow::Result;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::accounts::{load_accounts, resolve_password};
use crate::resolve;

use self::imap_sync::sync_account;
use self::manifest::generate_manifest;
use self::types::SyncState;

/// Load sync state from disk.
pub fn load_state() -> Result<SyncState> {
    let sf = resolve::sync_state_file();
    if sf.exists() {
        let data = std::fs::read(&sf)?;
        let state = types::load_state(&data)?;
        Ok(state)
    } else {
        Ok(SyncState::default())
    }
}

/// Save sync state to disk.
pub fn save_state(state: &SyncState) -> Result<()> {
    let data = serde_json::to_vec(state)?;
    std::fs::write(resolve::sync_state_file(), data)?;
    Ok(())
}

/// corky sync [--full] [--account NAME]
pub fn run(full: bool, account: Option<&str>) -> Result<()> {
    let accounts = load_accounts(None)?;
    let mut state = if full {
        SyncState::default()
    } else {
        load_state()?
    };

    let names: Vec<String> = if let Some(acct_name) = account {
        if !accounts.contains_key(acct_name) {
            anyhow::bail!(
                "Unknown account: {}\nAvailable: {}",
                acct_name,
                accounts.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        vec![acct_name.to_string()]
    } else {
        accounts.keys().cloned().collect()
    };

    // Track touched files for --full orphan cleanup
    let mut touched: Option<HashSet<PathBuf>> = if full { Some(HashSet::new()) } else { None };

    for name in &names {
        let acct = &accounts[name];
        println!("\n=== Account: {} ({}) ===", name, acct.user);

        match acct.provider.as_str() {
            "gmail-api" => {
                gmail_api_sync::sync_account(
                    name,
                    &acct.user,
                    &acct.labels,
                    acct.sync_days,
                    &mut state,
                    full,
                    touched.as_mut(),
                    None,
                )?;
            }
            _ => {
                let password = resolve_password(acct)?;
                sync_account(
                    name,
                    &acct.imap_host,
                    acct.imap_port,
                    acct.imap_starttls,
                    &acct.user,
                    &password,
                    &acct.labels,
                    acct.sync_days,
                    &mut state,
                    full,
                    None,
                    touched.as_mut(),
                    None,
                )?;
            }
        }
    }

    // Orphan cleanup on --full
    let conv_dir = resolve::conversations_dir();
    if let Some(ref touched_set) = touched {
        cleanup_orphans(&conv_dir, touched_set)?;
    }

    // Generate manifest
    generate_manifest(&conv_dir)?;

    save_state(&state)?;
    println!("\nSync complete.");
    Ok(())
}

/// Re-fetch a single thread by Gmail thread ID.
///
/// Finds the existing conversation file, fetches fresh message data via the
/// Gmail Threads API, deletes the old file, and re-merges all messages.
pub fn refetch(thread_id: &str) -> Result<()> {
    let conv_dir = resolve::conversations_dir();

    // Find existing thread file
    let thread_file = find_thread_file_by_id(&conv_dir, thread_id);

    // Parse existing file to get account and labels
    let (account_name, labels) = if let Some(ref path) = thread_file {
        let text = std::fs::read_to_string(path)?;
        let thread = markdown::parse_thread_markdown(&text);
        match thread {
            Some(t) => {
                let acct = t.accounts.first().cloned().unwrap_or_default();
                (acct, t.labels.clone())
            }
            None => (String::new(), vec![]),
        }
    } else {
        (String::new(), vec![])
    };

    if account_name.is_empty() {
        // No existing file or no account — try all gmail-api accounts
        let accounts = load_accounts(None)?;
        for (name, acct) in &accounts {
            if acct.provider == "gmail-api" {
                println!("Trying account: {}", name);
                match try_refetch_from_account(name, &acct.user, thread_id, &conv_dir, &[]) {
                    Ok(true) => {
                        manifest::generate_manifest(&conv_dir)?;
                        println!("\nRefetch complete.");
                        return Ok(());
                    }
                    Ok(false) => continue,
                    Err(e) => eprintln!("  Error: {}", e),
                }
            }
        }
        anyhow::bail!("Thread {} not found in any Gmail account", thread_id);
    }

    // Verify the account exists and is gmail-api
    let accounts = load_accounts(None)?;
    let acct = accounts.get(&account_name).ok_or_else(|| {
        anyhow::anyhow!("Account '{}' not found in .corky.toml", account_name)
    })?;
    if acct.provider != "gmail-api" {
        anyhow::bail!(
            "Account '{}' uses provider '{}', refetch only supports gmail-api",
            account_name,
            acct.provider
        );
    }

    // Delete existing file so merge creates fresh content
    if let Some(ref path) = thread_file {
        std::fs::remove_file(path)?;
        println!(
            "Removed: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    try_refetch_from_account(&account_name, &acct.user, thread_id, &conv_dir, &labels)?;

    // Also write to routed directories
    let label_routes = imap_sync::build_label_routes(&account_name);
    for l in &labels {
        if let Some(extra_dirs) = label_routes.get(l) {
            for extra_dir in extra_dirs {
                // Delete existing in routed dir too
                if let Some(routed_file) = find_thread_file_by_id(extra_dir, thread_id) {
                    std::fs::remove_file(&routed_file)?;
                }
                refetch_merge_to_dir(&account_name, l, thread_id, extra_dir)?;
            }
        }
    }

    manifest::generate_manifest(&conv_dir)?;
    println!("\nRefetch complete.");
    Ok(())
}

fn try_refetch_from_account(
    account_name: &str,
    user: &str,
    thread_id: &str,
    out_dir: &std::path::Path,
    labels: &[String],
) -> Result<bool> {
    use crate::filter::gmail_auth::{self, GMAIL_SYNC_SCOPE};

    let token = gmail_auth::get_access_token_for_user(Some(account_name), GMAIL_SYNC_SCOPE, Some(user))?;
    let messages = match gmail_api_sync::fetch_thread_messages(&token, thread_id) {
        Ok(msgs) => msgs,
        Err(e) => {
            let msg = format!("{}", e);
            if msg.contains("404") {
                return Ok(false);
            }
            return Err(e);
        }
    };

    if messages.is_empty() {
        println!("  Thread {} has no messages", thread_id);
        return Ok(false);
    }

    println!("  Fetched {} messages for thread {}", messages.len(), thread_id);

    let label = labels.first().map(|s| s.as_str()).unwrap_or("INBOX");
    for message in &messages {
        imap_sync::merge_message_to_file(out_dir, label, account_name, message, thread_id)?;
    }

    Ok(true)
}

fn refetch_merge_to_dir(
    account_name: &str,
    label: &str,
    thread_id: &str,
    out_dir: &std::path::Path,
) -> Result<()> {
    use crate::filter::gmail_auth::{self, GMAIL_SYNC_SCOPE};

    let accounts = load_accounts(None)?;
    let acct = accounts.get(account_name).ok_or_else(|| {
        anyhow::anyhow!("Account '{}' not found", account_name)
    })?;
    let token = gmail_auth::get_access_token_for_user(Some(account_name), GMAIL_SYNC_SCOPE, Some(&acct.user))?;
    let messages = gmail_api_sync::fetch_thread_messages(&token, thread_id)?;

    for message in &messages {
        imap_sync::merge_message_to_file(out_dir, label, account_name, message, thread_id)?;
    }

    Ok(())
}

/// Find a thread file by its Thread ID metadata.
fn find_thread_file_by_id(dir: &std::path::Path, thread_id: &str) -> Option<std::path::PathBuf> {
    use once_cell::sync::Lazy;
    use regex::Regex;

    static THREAD_ID_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^\*\*Thread ID\*\*:\s*(.+)$").unwrap());

    if !dir.exists() {
        return None;
    }
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Some(cap) = THREAD_ID_RE.captures(&text)
            && cap[1].trim() == thread_id
        {
            return Some(path);
        }
    }
    None
}

/// Delete conversation files not touched during a --full sync.
fn cleanup_orphans(conversations_dir: &PathBuf, touched: &HashSet<PathBuf>) -> Result<()> {
    if !conversations_dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(conversations_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") && !touched.contains(&path) {
            std::fs::remove_file(&path)?;
            println!(
                "  Removed orphan: {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }
    Ok(())
}
