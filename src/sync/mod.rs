//! IMAP email sync — fetch threads from IMAP and write to Markdown.

pub mod auth;
pub mod folders;
pub mod gmail_api_sync;
pub mod imap_sync;
pub mod imports;
pub mod manifest;
pub mod markdown;
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
