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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefetchTarget {
    original: String,
    lookup_id: String,
    search_query: Option<String>,
    is_gmail_url: bool,
    fetch_mode: RefetchFetchMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefetchFetchMode {
    Thread,
    SelectedMessage,
}

impl RefetchTarget {
    fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            anyhow::bail!("refetch target cannot be empty");
        }

        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            if !(lower.starts_with("https://mail.google.com/")
                || lower.starts_with("http://mail.google.com/"))
            {
                anyhow::bail!("refetch URL must be a Gmail URL from mail.google.com");
            }
            return parse_gmail_web_url(trimmed);
        }

        Ok(Self {
            original: trimmed.to_string(),
            lookup_id: trimmed.to_string(),
            search_query: None,
            is_gmail_url: false,
            fetch_mode: RefetchFetchMode::Thread,
        })
    }
}

fn parse_gmail_web_url(url: &str) -> Result<RefetchTarget> {
    if let Some(id) = extract_gmail_query_id(url) {
        return Ok(RefetchTarget {
            original: url.to_string(),
            lookup_id: id,
            search_query: None,
            is_gmail_url: true,
            fetch_mode: RefetchFetchMode::SelectedMessage,
        });
    }

    let Some((_, fragment)) = url.split_once('#') else {
        anyhow::bail!("Gmail URL does not contain a message or thread id fragment");
    };
    let fragment_path = fragment.split('?').next().unwrap_or(fragment);
    let segments: Vec<String> = fragment_path
        .split('/')
        .filter(|s| !s.trim().is_empty())
        .map(decode_url_component)
        .collect();

    let Some(id) = segments.last().cloned() else {
        anyhow::bail!("Gmail URL does not contain a message or thread id fragment");
    };

    let search_query =
        if segments.first().map(|s| s.as_str()) == Some("search") && segments.len() >= 3 {
            segments.get(1).cloned()
        } else {
            None
        };

    if is_gmail_view_name(&id) {
        anyhow::bail!("Gmail URL does not include a selected message or thread id");
    }

    Ok(RefetchTarget {
        original: url.to_string(),
        lookup_id: id,
        search_query,
        is_gmail_url: true,
        fetch_mode: RefetchFetchMode::SelectedMessage,
    })
}

fn extract_gmail_query_id(url: &str) -> Option<String> {
    let before_fragment = url.split('#').next().unwrap_or(url);
    let query = before_fragment.split_once('?')?.1;
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if matches!(key.as_ref(), "th" | "message_id" | "msg") && !value.trim().is_empty() {
            return Some(value.into_owned());
        }
    }
    None
}

fn decode_url_component(value: &str) -> String {
    let pair = format!("x={value}");
    form_urlencoded::parse(pair.as_bytes())
        .next()
        .map(|(_, decoded)| decoded.into_owned())
        .unwrap_or_else(|| value.to_string())
}

fn is_gmail_view_name(value: &str) -> bool {
    matches!(
        value,
        "inbox"
            | "starred"
            | "snoozed"
            | "sent"
            | "drafts"
            | "all"
            | "spam"
            | "trash"
            | "search"
            | "important"
            | "category"
    )
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

    // Orphan cleanup on --full of ALL accounts.
    //
    // #ckyorphan(a): a scoped `--account X` full sync only records files touched
    // for that one account, but conversations live in a single global
    // `mail/conversations/` dir. Running global cleanup then deletes every other
    // account's conversations. There is no reliable per-account orphan set
    // (threads accumulate labels across accounts), so a scoped sync skips
    // cleanup entirely. `cleanup_orphans` itself guards against reaping the
    // mailbox on a transient empty/partial fetch (#ckyorphan(b)).
    let conv_dir = resolve::conversations_dir();
    if let Some(ref touched_set) = touched {
        if account.is_some() {
            println!(
                "\nSkipping orphan cleanup: scoped --account sync cannot determine global orphans."
            );
        } else {
            cleanup_orphans(&conv_dir, touched_set)?;
        }
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

/// Re-fetch a Gmail thread by raw Gmail API ID, or a selected Gmail URL message.
///
/// Finds the existing conversation file, fetches fresh message data via the
/// Gmail API, deletes the old file, and re-merges the requested message set.
pub fn refetch(thread_id_or_url: &str) -> Result<()> {
    refetch_internal(thread_id_or_url, false).map(|_| ())
}

pub fn refetch_report(thread_id_or_url: &str) -> Result<RefetchReport> {
    refetch_internal(thread_id_or_url, true)
}

fn refetch_internal(thread_id_or_url: &str, quiet: bool) -> Result<RefetchReport> {
    let target = RefetchTarget::parse(thread_id_or_url)?;
    let conv_dir = resolve::conversations_dir();
    let mut attempted_accounts = Vec::new();
    let mut removed_files = Vec::new();

    // Find existing thread file
    let thread_file = find_thread_file_by_id(&conv_dir, &target.lookup_id);

    // Parse existing file to get account and labels
    let (account_name, labels) = if let Some(ref path) = thread_file {
        read_thread_file_context(path)?
    } else {
        (String::new(), vec![])
    };

    if target.is_gmail_url && !quiet {
        println!("Looking up Gmail URL id: {}", target.lookup_id);
        if let Some(ref query) = target.search_query {
            println!("  Search context: {}", query);
        }
    }

    if account_name.is_empty() {
        // No existing file or no account — try all gmail-api accounts
        let accounts = load_accounts(None)?;
        for (name, acct) in &accounts {
            if acct.provider == "gmail-api" {
                attempted_accounts.push(name.clone());
                if !quiet {
                    println!("Trying account: {}", name);
                }
                match try_fetch_from_account(name, &acct.user, &target, quiet) {
                    Ok(Some(fetch)) => {
                        let resolved_thread_file =
                            find_thread_file_by_id(&conv_dir, &fetch.thread_id);
                        let labels = if let Some(ref path) = resolved_thread_file {
                            let (_, labels) = read_thread_file_context(path)?;
                            labels
                        } else {
                            vec![]
                        };
                        let existing_file = remove_existing_thread_files(
                            &conv_dir,
                            &fetch.thread_id,
                            thread_file.as_deref(),
                            &mut removed_files,
                            quiet,
                        )?;
                        let messages_fetched =
                            merge_fetched_thread(name, &labels, &fetch, &conv_dir)?;
                        let routed_refresh_count =
                            refresh_routed_dirs(name, &labels, &fetch, &mut removed_files, quiet)?;
                        manifest::generate_manifest(&conv_dir)?;
                        if !quiet {
                            println!("\nRefetch complete.");
                        }
                        return Ok(RefetchReport {
                            thread_id: fetch.thread_id,
                            account_name: Some(name.clone()),
                            labels,
                            existing_file,
                            removed_files,
                            messages_fetched,
                            routed_refresh_count,
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
        if target.is_gmail_url {
            anyhow::bail!(
                "Gmail URL target {} not found in any Gmail API account",
                target.lookup_id
            );
        }
        anyhow::bail!("Thread {} not found in any Gmail account", target.lookup_id);
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

    let fetch =
        try_fetch_from_account(&account_name, &acct.user, &target, quiet)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Thread {} not found for account {}",
                target.lookup_id,
                account_name
            )
        })?;

    let resolved_thread_file = find_thread_file_by_id(&conv_dir, &fetch.thread_id);
    let labels = if let Some(ref path) = resolved_thread_file {
        let (_, labels) = read_thread_file_context(path)?;
        labels
    } else {
        labels
    };

    let existing_file = remove_existing_thread_files(
        &conv_dir,
        &fetch.thread_id,
        thread_file.as_deref(),
        &mut removed_files,
        quiet,
    )?;
    let messages_fetched = merge_fetched_thread(&account_name, &labels, &fetch, &conv_dir)?;
    let routed_refresh_count =
        refresh_routed_dirs(&account_name, &labels, &fetch, &mut removed_files, quiet)?;

    manifest::generate_manifest(&conv_dir)?;
    if !quiet {
        println!("\nRefetch complete.");
    }
    Ok(RefetchReport {
        thread_id: fetch.thread_id,
        account_name: Some(account_name),
        labels,
        existing_file,
        removed_files,
        messages_fetched,
        routed_refresh_count,
        attempted_accounts,
    })
}

fn try_fetch_from_account(
    account_name: &str,
    user: &str,
    target: &RefetchTarget,
    quiet: bool,
) -> Result<Option<gmail_api_sync::ThreadFetch>> {
    use crate::filter::gmail_auth::{self, GMAIL_SYNC_SCOPE};

    let token =
        gmail_auth::get_access_token_for_user(Some(account_name), GMAIL_SYNC_SCOPE, Some(user))?;
    let fetch = match target.fetch_mode {
        RefetchFetchMode::Thread => gmail_api_sync::fetch_thread_by_ref(&token, &target.lookup_id)?,
        RefetchFetchMode::SelectedMessage => {
            gmail_api_sync::fetch_selected_message_by_ref(&token, &target.lookup_id)?
        }
    };
    let Some(fetch) = fetch else {
        return Ok(None);
    };

    if fetch.messages.is_empty() {
        if !quiet {
            println!("  Thread {} has no messages", fetch.thread_id);
        }
        return Ok(None);
    }

    if !quiet {
        println!(
            "  Fetched {} {} for thread {}",
            fetch.messages.len(),
            match target.fetch_mode {
                RefetchFetchMode::Thread => "messages",
                RefetchFetchMode::SelectedMessage => "selected message",
            },
            fetch.thread_id,
        );
        if target.is_gmail_url && target.lookup_id != fetch.thread_id {
            println!(
                "  Resolved Gmail URL id {} -> {}",
                target.lookup_id, fetch.thread_id
            );
        }
    }

    Ok(Some(fetch))
}

fn merge_fetched_thread(
    account_name: &str,
    labels: &[String],
    fetch: &gmail_api_sync::ThreadFetch,
    out_dir: &std::path::Path,
) -> Result<usize> {
    let label = labels.first().map(|s| s.as_str()).unwrap_or("INBOX");

    for message in &fetch.messages {
        imap_sync::merge_message_to_file(out_dir, label, account_name, message, &fetch.thread_id)?;
    }

    Ok(fetch.messages.len())
}

fn refresh_routed_dirs(
    account_name: &str,
    labels: &[String],
    fetch: &gmail_api_sync::ThreadFetch,
    removed_files: &mut Vec<String>,
    quiet: bool,
) -> Result<usize> {
    let label_routes = imap_sync::build_label_routes(account_name);
    let mut routed_refresh_count = 0usize;
    for label in labels {
        if let Some(extra_dirs) = label_routes.get(label) {
            for extra_dir in extra_dirs {
                if let Some(routed_file) = find_thread_file_by_id(extra_dir, &fetch.thread_id) {
                    remove_thread_file(&routed_file, removed_files, quiet)?;
                }
                merge_fetched_thread(account_name, std::slice::from_ref(label), fetch, extra_dir)?;
                routed_refresh_count += 1;
            }
        }
    }
    Ok(routed_refresh_count)
}

fn read_thread_file_context(path: &std::path::Path) -> Result<(String, Vec<String>)> {
    let text = std::fs::read_to_string(path)?;
    let thread = markdown::parse_thread_markdown(&text);
    match thread {
        Some(t) => {
            let acct = t.accounts.first().cloned().unwrap_or_default();
            Ok((acct, t.labels.clone()))
        }
        None => Ok((String::new(), vec![])),
    }
}

fn remove_existing_thread_files(
    conv_dir: &std::path::Path,
    resolved_thread_id: &str,
    original_thread_file: Option<&std::path::Path>,
    removed_files: &mut Vec<String>,
    quiet: bool,
) -> Result<Option<String>> {
    let resolved_thread_file = find_thread_file_by_id(conv_dir, resolved_thread_id);
    let existing_file = resolved_thread_file
        .as_deref()
        .or(original_thread_file)
        .map(|path| path.display().to_string());

    if let Some(path) = original_thread_file {
        remove_thread_file(path, removed_files, quiet)?;
    }
    if let Some(path) = resolved_thread_file.as_deref()
        && Some(path) != original_thread_file
    {
        remove_thread_file(path, removed_files, quiet)?;
    }

    Ok(existing_file)
}

fn remove_thread_file(
    path: &std::path::Path,
    removed_files: &mut Vec<String>,
    quiet: bool,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    std::fs::remove_file(path)?;
    removed_files.push(path.display().to_string());
    if !quiet {
        println!(
            "Removed: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
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

/// Fraction of the mailbox that a single `--full` sync may reap before the
/// safety guard trips. A full sync that would delete more than this share of
/// existing conversations is treated as a likely transient/partial fetch
/// failure rather than a genuine mass deletion, and is skipped. Override with
/// `CORKY_SYNC_FORCE_ORPHAN_CLEANUP=1`.
const ORPHAN_CLEANUP_MAX_DELETE_FRACTION: f64 = 0.5;

fn force_orphan_cleanup() -> bool {
    std::env::var("CORKY_SYNC_FORCE_ORPHAN_CLEANUP")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Delete conversation files not touched during a `--full` sync.
///
/// #ckyorphan(b): a transient empty/partial fetch leaves `touched` empty (or
/// far smaller than the mailbox), which would classify every — or nearly every —
/// conversation as an orphan and wipe the data store. Guard against that: if the
/// sync would delete more than [`ORPHAN_CLEANUP_MAX_DELETE_FRACTION`] of the
/// mailbox (which includes the `touched.is_empty()` case), skip and warn instead
/// of reaping, leaving the files in place so the next successful sync recovers.
/// `CORKY_SYNC_FORCE_ORPHAN_CLEANUP=1` overrides the guard for intentional bulk
/// deletions.
fn cleanup_orphans(conversations_dir: &PathBuf, touched: &HashSet<PathBuf>) -> Result<()> {
    if !conversations_dir.exists() {
        return Ok(());
    }

    let mut md_files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(conversations_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            md_files.push(path);
        }
    }

    let total = md_files.len();
    let orphans: Vec<PathBuf> = md_files
        .into_iter()
        .filter(|path| !touched.contains(path))
        .collect();

    if orphans.is_empty() {
        return Ok(());
    }

    // Safety guard: never reap the overwhelming majority of the mailbox in one
    // sync unless explicitly forced — that pattern means a transient/partial
    // fetch (or a scoped sync that slipped through), not a real deletion.
    if !force_orphan_cleanup() && total > 0 {
        let delete_fraction = orphans.len() as f64 / total as f64;
        if delete_fraction > ORPHAN_CLEANUP_MAX_DELETE_FRACTION {
            eprintln!(
                "  Skipping orphan cleanup: would delete {}/{} conversations ({:.0}%), above the {:.0}% safety threshold.\n  \
This usually means a transient or partial fetch. Re-run a successful full sync, or set \
CORKY_SYNC_FORCE_ORPHAN_CLEANUP=1 to override.",
                orphans.len(),
                total,
                delete_fraction * 100.0,
                ORPHAN_CLEANUP_MAX_DELETE_FRACTION * 100.0
            );
            return Ok(());
        }
    }

    for path in orphans {
        std::fs::remove_file(&path)?;
        println!(
            "  Removed orphan: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::{AccountSyncState, ContactSyncState, GmailLabelState, LabelState};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("corky-orphan-test-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_md(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "x").unwrap();
        path
    }

    #[test]
    fn cleanup_orphans_preserves_all_when_touched_empty() {
        // #ckyorphan(b): a transient empty fetch (touched empty) must NOT reap
        // the whole mailbox.
        let dir = unique_temp_dir("empty");
        let a = write_md(&dir, "a.md");
        let b = write_md(&dir, "b.md");
        cleanup_orphans(&dir, &HashSet::new()).unwrap();
        assert!(a.exists() && b.exists(), "empty touched must not delete files");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cleanup_orphans_skips_when_majority_orphaned() {
        // Touching only 1 of 4 (75% would be reaped) trips the safety guard.
        let dir = unique_temp_dir("majority");
        let keep = write_md(&dir, "keep.md");
        let o1 = write_md(&dir, "o1.md");
        let o2 = write_md(&dir, "o2.md");
        let o3 = write_md(&dir, "o3.md");
        let touched: HashSet<PathBuf> = [keep.clone()].into_iter().collect();
        cleanup_orphans(&dir, &touched).unwrap();
        assert!(
            keep.exists() && o1.exists() && o2.exists() && o3.exists(),
            "above-threshold reap must be skipped"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cleanup_orphans_deletes_below_threshold() {
        // Touching 3 of 4 (25% reaped) is below the guard → orphan is deleted.
        let dir = unique_temp_dir("below");
        let k1 = write_md(&dir, "k1.md");
        let k2 = write_md(&dir, "k2.md");
        let k3 = write_md(&dir, "k3.md");
        let orphan = write_md(&dir, "orphan.md");
        let touched: HashSet<PathBuf> = [k1.clone(), k2.clone(), k3.clone()].into_iter().collect();
        cleanup_orphans(&dir, &touched).unwrap();
        assert!(k1.exists() && k2.exists() && k3.exists());
        assert!(!orphan.exists(), "below-threshold orphan should be deleted");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_refetch_target_keeps_raw_thread_id() {
        let target = RefetchTarget::parse("19d479af292d8d99").unwrap();

        assert_eq!(target.lookup_id, "19d479af292d8d99");
        assert_eq!(target.search_query, None);
        assert!(!target.is_gmail_url);
        assert_eq!(target.fetch_mode, RefetchFetchMode::Thread);
    }

    #[test]
    fn parse_refetch_target_extracts_gmail_search_url_id() {
        let target = RefetchTarget::parse(
            "https://mail.google.com/mail/u/0/#search/philip/FMfcgzQgLsCwhwtlQXPkzpdLhKhzPdNv",
        )
        .unwrap();

        assert_eq!(target.lookup_id, "FMfcgzQgLsCwhwtlQXPkzpdLhKhzPdNv");
        assert_eq!(target.search_query, Some("philip".to_string()));
        assert!(target.is_gmail_url);
        assert_eq!(target.fetch_mode, RefetchFetchMode::SelectedMessage);
    }

    #[test]
    fn parse_refetch_target_extracts_gmail_inbox_url_id() {
        let target =
            RefetchTarget::parse("https://mail.google.com/mail/u/1/#inbox/18abc123def456").unwrap();

        assert_eq!(target.lookup_id, "18abc123def456");
        assert_eq!(target.search_query, None);
        assert!(target.is_gmail_url);
        assert_eq!(target.fetch_mode, RefetchFetchMode::SelectedMessage);
    }

    #[test]
    fn parse_refetch_target_extracts_query_thread_id() {
        let target =
            RefetchTarget::parse("https://mail.google.com/mail/u/0/?th=18abc123def456#inbox")
                .unwrap();

        assert_eq!(target.lookup_id, "18abc123def456");
        assert!(target.is_gmail_url);
        assert_eq!(target.fetch_mode, RefetchFetchMode::SelectedMessage);
    }

    #[test]
    fn parse_refetch_target_rejects_gmail_view_without_id() {
        let err = RefetchTarget::parse("https://mail.google.com/mail/u/0/#inbox").unwrap_err();

        assert!(
            err.to_string()
                .contains("does not include a selected message or thread id")
        );
    }

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
