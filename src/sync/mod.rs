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
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::accounts::{load_accounts, resolve_password};
use crate::file_store;
use crate::resolve;

use self::imap_sync::sync_account;
use self::manifest::generate_manifest;
use self::types::SyncState;

#[derive(Debug, Clone, Serialize)]
pub struct RefetchReport {
    pub thread_id: String,
    pub account_name: Option<String>,
    pub labels: Vec<String>,
    pub existing_file: Option<String>,
    pub removed_files: Vec<String>,
    pub messages_fetched: usize,
    pub routed_refresh_count: usize,
    pub attempted_accounts: Vec<String>,
}

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
    let path = resolve::sync_state_file();
    file_store::save_json_with_lock::<SyncState, _>(&path, None, |_| Ok(state.clone()))
}

/// Save sync state to disk, preserving concurrent updates to untouched sections.
pub fn save_state_merged(base: &SyncState, updated: &SyncState) -> Result<()> {
    let path = resolve::sync_state_file();
    file_store::save_json_with_lock::<SyncState, _>(&path, None, |current| {
        Ok(merge_sync_states(base, &current, updated))
    })
}

/// corky sync [--full] [--account NAME]
pub fn run(full: bool, account: Option<&str>) -> Result<()> {
    let accounts = load_accounts(None)?;
    let base_state = if full {
        SyncState::default()
    } else {
        load_state()?
    };
    let mut state = base_state.clone();

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

    save_state_merged(&base_state, &state)?;
    println!("\nSync complete.");
    Ok(())
}

fn merge_sync_states(base: &SyncState, current: &SyncState, updated: &SyncState) -> SyncState {
    SyncState {
        accounts: merge_account_states(&base.accounts, &current.accounts, &updated.accounts),
        contacts: merge_contact_states(&base.contacts, &current.contacts, &updated.contacts),
    }
}

fn merge_account_states(
    base: &HashMap<String, types::AccountSyncState>,
    current: &HashMap<String, types::AccountSyncState>,
    updated: &HashMap<String, types::AccountSyncState>,
) -> HashMap<String, types::AccountSyncState> {
    let mut merged = current.clone();
    for (name, updated_account) in updated {
        let base_account = base.get(name);
        let current_account = current.get(name);

        if current_account == Some(updated_account) || base_account == Some(updated_account) {
            continue;
        }

        let next = match (base_account, current_account) {
            (_, None) => updated_account.clone(),
            (Some(base_account), Some(current_account)) if current_account == base_account => {
                updated_account.clone()
            }
            (Some(base_account), Some(current_account)) => types::AccountSyncState {
                labels: merge_label_states(
                    &base_account.labels,
                    &current_account.labels,
                    &updated_account.labels,
                ),
                gmail_labels: merge_gmail_label_states(
                    &base_account.gmail_labels,
                    &current_account.gmail_labels,
                    &updated_account.gmail_labels,
                ),
            },
            (None, Some(current_account)) => {
                merge_account_without_base(current_account, updated_account)
            }
        };

        merged.insert(name.clone(), next);
    }
    merged
}

fn merge_account_without_base(
    current: &types::AccountSyncState,
    updated: &types::AccountSyncState,
) -> types::AccountSyncState {
    types::AccountSyncState {
        labels: merge_label_states(&HashMap::new(), &current.labels, &updated.labels),
        gmail_labels: merge_gmail_label_states(
            &HashMap::new(),
            &current.gmail_labels,
            &updated.gmail_labels,
        ),
    }
}

fn merge_label_states(
    base: &HashMap<String, types::LabelState>,
    current: &HashMap<String, types::LabelState>,
    updated: &HashMap<String, types::LabelState>,
) -> HashMap<String, types::LabelState> {
    let mut merged = current.clone();
    for (label, updated_state) in updated {
        let base_state = base.get(label);
        let current_state = current.get(label);

        if current_state == Some(updated_state) || base_state == Some(updated_state) {
            continue;
        }

        let next = match (base_state, current_state) {
            (_, None) => updated_state.clone(),
            (Some(base_state), Some(current_state)) if current_state == base_state => {
                updated_state.clone()
            }
            (_, Some(current_state)) if current_state.uidvalidity == updated_state.uidvalidity => {
                types::LabelState {
                    uidvalidity: current_state.uidvalidity,
                    last_uid: current_state.last_uid.max(updated_state.last_uid),
                }
            }
            _ => updated_state.clone(),
        };

        merged.insert(label.clone(), next);
    }
    merged
}

fn merge_gmail_label_states(
    base: &HashMap<String, types::GmailLabelState>,
    current: &HashMap<String, types::GmailLabelState>,
    updated: &HashMap<String, types::GmailLabelState>,
) -> HashMap<String, types::GmailLabelState> {
    let mut merged = current.clone();
    for (label, updated_state) in updated {
        let base_state = base.get(label);
        let current_state = current.get(label);

        if current_state == Some(updated_state) || base_state == Some(updated_state) {
            continue;
        }

        let next = match (base_state, current_state) {
            (_, None) => updated_state.clone(),
            (Some(base_state), Some(current_state)) if current_state == base_state => {
                updated_state.clone()
            }
            (_, Some(current_state)) => types::GmailLabelState {
                last_history_id: match (
                    current_state.last_history_id,
                    updated_state.last_history_id,
                ) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (Some(a), None) => Some(a),
                    (None, Some(b)) => Some(b),
                    (None, None) => None,
                },
            },
        };

        merged.insert(label.clone(), next);
    }
    merged
}

fn merge_contact_states(
    base: &HashMap<String, types::ContactSyncState>,
    current: &HashMap<String, types::ContactSyncState>,
    updated: &HashMap<String, types::ContactSyncState>,
) -> HashMap<String, types::ContactSyncState> {
    let mut merged = current.clone();
    for (contact, updated_state) in updated {
        let base_state = base.get(contact);
        let current_state = current.get(contact);

        if current_state == Some(updated_state) || base_state == Some(updated_state) {
            continue;
        }

        let next = match (base_state, current_state) {
            (_, None) => updated_state.clone(),
            (Some(base_state), Some(current_state)) if current_state == base_state => {
                updated_state.clone()
            }
            (_, Some(current_state)) => types::ContactSyncState {
                mailboxes: merge_contact_mailboxes(
                    base_state.map(|state| &state.mailboxes),
                    &current_state.mailboxes,
                    &updated_state.mailboxes,
                ),
            },
        };

        merged.insert(contact.clone(), next);
    }
    merged
}

fn merge_contact_mailboxes(
    base: Option<&HashMap<String, String>>,
    current: &HashMap<String, String>,
    updated: &HashMap<String, String>,
) -> HashMap<String, String> {
    let empty = HashMap::new();
    let mut merged = current.clone();
    for (mailbox, updated_hash) in updated {
        let base_hash = base.unwrap_or(&empty).get(mailbox);
        let current_hash = current.get(mailbox);

        if current_hash == Some(updated_hash) || base_hash == Some(updated_hash) {
            continue;
        }

        if base_hash.is_none() || current_hash == base_hash {
            merged.insert(mailbox.clone(), updated_hash.clone());
        }
    }
    merged
}

/// Re-fetch a single thread by Gmail thread ID.
///
/// Finds the existing conversation file, fetches fresh message data via the
/// Gmail Threads API, deletes the old file, and re-merges all messages.
pub fn refetch(thread_id: &str) -> Result<()> {
    refetch_internal(thread_id, false).map(|_| ())
}

pub fn refetch_report(thread_id: &str) -> Result<RefetchReport> {
    refetch_internal(thread_id, true)
}

fn refetch_internal(thread_id: &str, quiet: bool) -> Result<RefetchReport> {
    let conv_dir = resolve::conversations_dir();
    let mut attempted_accounts = Vec::new();
    let mut removed_files = Vec::new();

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
                attempted_accounts.push(name.clone());
                if !quiet {
                    println!("Trying account: {}", name);
                }
                match try_refetch_from_account(name, &acct.user, thread_id, &conv_dir, &[], quiet) {
                    Ok(Some(messages_fetched)) => {
                        manifest::generate_manifest(&conv_dir)?;
                        if !quiet {
                            println!("\nRefetch complete.");
                        }
                        return Ok(RefetchReport {
                            thread_id: thread_id.to_string(),
                            account_name: Some(name.clone()),
                            labels: vec![],
                            existing_file: None,
                            removed_files,
                            messages_fetched,
                            routed_refresh_count: 0,
                            attempted_accounts,
                        });
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        if !quiet {
                            eprintln!("  Error: {}", e);
                        }
                    }
                }
            }
        }
        anyhow::bail!("Thread {} not found in any Gmail account", thread_id);
    }

    // Verify the account exists and is gmail-api
    let accounts = load_accounts(None)?;
    let acct = accounts
        .get(&account_name)
        .ok_or_else(|| anyhow::anyhow!("Account '{}' not found in .corky.toml", account_name))?;
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
        removed_files.push(path.display().to_string());
        if !quiet {
            println!(
                "Removed: {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }

    let messages_fetched = try_refetch_from_account(
        &account_name,
        &acct.user,
        thread_id,
        &conv_dir,
        &labels,
        quiet,
    )?
    .ok_or_else(|| {
        anyhow::anyhow!(
            "Thread {} not found for account {}",
            thread_id,
            account_name
        )
    })?;

    // Also write to routed directories
    let label_routes = imap_sync::build_label_routes(&account_name);
    let mut routed_refresh_count = 0usize;
    for l in &labels {
        if let Some(extra_dirs) = label_routes.get(l) {
            for extra_dir in extra_dirs {
                // Delete existing in routed dir too
                if let Some(routed_file) = find_thread_file_by_id(extra_dir, thread_id) {
                    std::fs::remove_file(&routed_file)?;
                    removed_files.push(routed_file.display().to_string());
                }
                refetch_merge_to_dir(&account_name, l, thread_id, extra_dir)?;
                routed_refresh_count += 1;
            }
        }
    }

    manifest::generate_manifest(&conv_dir)?;
    if !quiet {
        println!("\nRefetch complete.");
    }
    Ok(RefetchReport {
        thread_id: thread_id.to_string(),
        account_name: Some(account_name),
        labels,
        existing_file: thread_file.map(|p| p.display().to_string()),
        removed_files,
        messages_fetched,
        routed_refresh_count,
        attempted_accounts,
    })
}

fn try_refetch_from_account(
    account_name: &str,
    user: &str,
    thread_id: &str,
    out_dir: &std::path::Path,
    labels: &[String],
    quiet: bool,
) -> Result<Option<usize>> {
    use crate::filter::gmail_auth::{self, GMAIL_SYNC_SCOPE};

    let token =
        gmail_auth::get_access_token_for_user(Some(account_name), GMAIL_SYNC_SCOPE, Some(user))?;
    let messages = match gmail_api_sync::fetch_thread_messages(&token, thread_id) {
        Ok(msgs) => msgs,
        Err(e) => {
            let msg = format!("{}", e);
            if msg.contains("404") {
                return Ok(None);
            }
            return Err(e);
        }
    };

    if messages.is_empty() {
        if !quiet {
            println!("  Thread {} has no messages", thread_id);
        }
        return Ok(None);
    }

    if !quiet {
        println!(
            "  Fetched {} messages for thread {}",
            messages.len(),
            thread_id
        );
    }

    let label = labels.first().map(|s| s.as_str()).unwrap_or("INBOX");
    for message in &messages {
        imap_sync::merge_message_to_file(out_dir, label, account_name, message, thread_id)?;
    }

    Ok(Some(messages.len()))
}

fn refetch_merge_to_dir(
    account_name: &str,
    label: &str,
    thread_id: &str,
    out_dir: &std::path::Path,
) -> Result<()> {
    use crate::filter::gmail_auth::{self, GMAIL_SYNC_SCOPE};

    let accounts = load_accounts(None)?;
    let acct = accounts
        .get(account_name)
        .ok_or_else(|| anyhow::anyhow!("Account '{}' not found", account_name))?;
    let token = gmail_auth::get_access_token_for_user(
        Some(account_name),
        GMAIL_SYNC_SCOPE,
        Some(&acct.user),
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::{AccountSyncState, ContactSyncState, GmailLabelState, LabelState};

    fn label(uidvalidity: u32, last_uid: u32) -> LabelState {
        LabelState {
            uidvalidity,
            last_uid,
        }
    }

    fn account(labels: &[(&str, u32, u32)]) -> AccountSyncState {
        let mut state = AccountSyncState::default();
        for (name, uidvalidity, last_uid) in labels {
            state
                .labels
                .insert((*name).to_string(), label(*uidvalidity, *last_uid));
        }
        state
    }

    #[test]
    fn merge_sync_states_preserves_concurrent_contact_updates() {
        let mut base = SyncState::default();
        base.accounts
            .insert("gmail".to_string(), account(&[("INBOX", 1, 100)]));

        let mut current = base.clone();
        current.contacts.insert(
            "alice".to_string(),
            ContactSyncState {
                mailboxes: HashMap::from([("personal".to_string(), "hash-a".to_string())]),
            },
        );

        let mut updated = base.clone();
        updated
            .accounts
            .insert("gmail".to_string(), account(&[("INBOX", 1, 125)]));

        let merged = merge_sync_states(&base, &current, &updated);
        assert_eq!(merged.accounts["gmail"].labels["INBOX"].last_uid, 125);
        assert_eq!(merged.contacts["alice"].mailboxes["personal"], "hash-a");
    }

    #[test]
    fn merge_sync_states_takes_max_uid_for_same_label() {
        let mut base = SyncState::default();
        base.accounts
            .insert("gmail".to_string(), account(&[("INBOX", 1, 100)]));

        let mut current = base.clone();
        current
            .accounts
            .insert("gmail".to_string(), account(&[("INBOX", 1, 105)]));

        let mut updated = base.clone();
        updated
            .accounts
            .insert("gmail".to_string(), account(&[("INBOX", 1, 110)]));

        let merged = merge_sync_states(&base, &current, &updated);
        assert_eq!(merged.accounts["gmail"].labels["INBOX"].last_uid, 110);
    }

    #[test]
    fn merge_sync_states_takes_max_history_id_for_same_gmail_label() {
        let mut base = SyncState::default();
        let mut base_account = AccountSyncState::default();
        base_account
            .labels
            .insert("INBOX".to_string(), label(1, 100));
        base_account.gmail_labels.insert(
            "INBOX".to_string(),
            GmailLabelState {
                last_history_id: Some(200),
            },
        );
        base.accounts.insert("gmail".to_string(), base_account);

        let mut current = base.clone();
        current
            .accounts
            .get_mut("gmail")
            .unwrap()
            .gmail_labels
            .insert(
                "INBOX".to_string(),
                GmailLabelState {
                    last_history_id: Some(220),
                },
            );

        let mut updated = base.clone();
        updated
            .accounts
            .get_mut("gmail")
            .unwrap()
            .gmail_labels
            .insert(
                "INBOX".to_string(),
                GmailLabelState {
                    last_history_id: Some(240),
                },
            );

        let merged = merge_sync_states(&base, &current, &updated);
        assert_eq!(
            merged.accounts["gmail"].gmail_labels["INBOX"].last_history_id,
            Some(240)
        );
    }
}
