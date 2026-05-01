//! IMAP polling daemon — syncs email and pushes to shared repos on an interval.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::accounts::{load_accounts, load_watch_config, resolve_password};
use crate::config::corky_config;
use crate::desktop_notify::notify;
use crate::resolve;
use crate::sync::gmail_api_sync;
use crate::sync::imap_sync::sync_account;
use crate::sync::types::SyncState;

/// Snapshot {account: {label: last_uid}} from current sync state.
fn snapshot_uids(state: &SyncState) -> HashMap<String, HashMap<String, u32>> {
    let mut snap = HashMap::new();
    for (acct_name, acct_state) in &state.accounts {
        let mut labels = HashMap::new();
        for (label, ls) in &acct_state.labels {
            labels.insert(label.clone(), ls.last_uid);
        }
        snap.insert(acct_name.clone(), labels);
    }
    snap
}

/// Count labels where last_uid increased.
fn count_new_messages(
    before: &HashMap<String, HashMap<String, u32>>,
    after: &HashMap<String, HashMap<String, u32>>,
) -> usize {
    let mut count = 0;
    for (acct_name, labels) in after {
        let before_acct = before.get(acct_name);
        for (label, uid) in labels {
            let before_uid = before_acct.and_then(|a| a.get(label)).copied().unwrap_or(0);
            if *uid > before_uid {
                count += 1;
            }
        }
    }
    count
}

fn load_state() -> SyncState {
    crate::sync::load_state().unwrap_or_default()
}

fn save_state(base: &SyncState, state: &SyncState) {
    let _ = crate::sync::save_state_merged(base, state);
}

fn sync_mailboxes() {
    let config = match corky_config::try_load_config(None) {
        Some(c) => c,
        None => return,
    };
    for name in config.mailboxes.keys() {
        let mb_path = resolve::mailbox_dir(name);
        if !mb_path.exists() || !mb_path.join(".git").exists() {
            continue;
        }
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(mb_path.to_string_lossy().as_ref())
            .arg("status")
            .arg("--porcelain")
            .output();
        if let Ok(out) = output
            && !String::from_utf8_lossy(&out.stdout).trim().is_empty()
        {
            let _ = crate::mailbox::sync::sync_one(name);
        }
    }
}

/// Run pending scheduled items (best-effort, never crashes the watch loop).
fn schedule_tick() {
    if let Err(e) = crate::schedule::run(false) {
        eprintln!("schedule: {}", e);
    }
}

/// Check for upgrade and self-restart if a newer version is available.
/// Returns true if the process should restart (exec failed as fallback).
fn try_auto_upgrade() -> bool {
    let latest = match crate::upgrade::check_for_update() {
        Some(v) => v,
        None => return false,
    };

    eprintln!(
        "\ncorky watch: upgrading {} → {}...",
        env!("CARGO_PKG_VERSION"),
        latest
    );

    if let Err(e) = crate::upgrade::run() {
        eprintln!("Auto-upgrade failed: {}", e);
        return false;
    }

    eprintln!("corky watch: restarting with new version...");

    // Re-exec self with the same arguments
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(_) => return false,
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        let err = std::process::Command::new(exe).args(&args).exec();
        // exec() only returns on error
        eprintln!("exec failed: {}", err);
    }

    false
}

/// Check for Gmail filter drift (best-effort, never crashes the watch loop).
/// Uses non-interactive auth — never opens a browser.
fn check_filter_drift() {
    match crate::filter::check::run_noninteractive(None) {
        Ok(true) => {} // in sync, no output needed
        Ok(false) => {
            eprintln!("corky watch: filter drift detected — run `corky filter push` to sync");
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Run `corky filter auth`") {
                eprintln!("corky watch: {}", msg);
            } else if !msg.contains("No [gmail] section") && !msg.contains("not found at") {
                eprintln!("corky watch: filter check failed: {}", msg);
            }
        }
    }
}

/// One sync + mailbox sync cycle. Returns count of labels with new messages.
fn poll_once(notify_enabled: bool, shutdown: Arc<AtomicBool>) -> usize {
    let accounts = match load_accounts(None) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Failed to load accounts: {}", e);
            return 0;
        }
    };

    let base_state = load_state();
    let mut state = base_state.clone();
    let before = snapshot_uids(&state);

    for (acct_name, acct) in &accounts {
        println!("\n=== Account: {} ({}) ===", acct_name, acct.user);

        let result = match acct.provider.as_str() {
            "gmail-api" => gmail_api_sync::sync_account(
                acct_name,
                &acct.user,
                &acct.labels,
                acct.sync_days,
                &mut state,
                false,
                None,
                Some(&shutdown),
            ),
            _ => {
                let password = match resolve_password(acct) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("  Error resolving password for {}: {}", acct_name, e);
                        continue;
                    }
                };
                sync_account(
                    acct_name,
                    &acct.imap_host,
                    acct.imap_port,
                    acct.imap_starttls,
                    &acct.user,
                    &password,
                    &acct.labels,
                    acct.sync_days,
                    &mut state,
                    false,
                    None,
                    None,
                    Some(&shutdown),
                )
            }
        };
        if let Err(e) = result {
            eprintln!("  Error syncing {}: {}", acct_name, e);
            continue;
        }
    }

    save_state(&base_state, &state);

    let after = snapshot_uids(&state);
    let new_count = count_new_messages(&before, &after);

    if new_count > 0 {
        println!("\n{} label(s) with new messages", new_count);
        sync_mailboxes();
        if notify_enabled {
            notify(
                "corky",
                &format!("{} label(s) with new messages", new_count),
            );
        }
    } else {
        println!("\nNo new messages");
    }

    new_count
}

/// corky watch [--interval N]
#[tokio::main]
pub async fn run(interval_override: Option<u64>) -> Result<()> {
    let config = load_watch_config(None)?;
    let interval = interval_override.unwrap_or(config.poll_interval);

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Handle Ctrl-C — set flag and notify via channel for immediate wakeup
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        println!("\nReceived signal, shutting down...");
        shutdown_clone.store(true, Ordering::Relaxed);
        let _ = shutdown_tx.send(true);
    });

    let auto_upgrade = config.auto_upgrade;
    println!(
        "corky watch: polling every {}s{} (Ctrl-C to stop)",
        interval,
        if auto_upgrade {
            ", auto-upgrade on"
        } else {
            ""
        }
    );

    let mut cycles_since_upgrade_check: u64 = 0;
    let mut cycles_since_filter_check: u64 = 0;
    // Check for upgrades every N cycles (roughly once per hour)
    let upgrade_check_every = (3600 / interval).max(1);
    // Check filter drift every N cycles (roughly once per hour)
    let filter_check_every = upgrade_check_every;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Run sync in a blocking context
        let notify_enabled = config.notify;
        let shutdown_for_poll = shutdown.clone();
        tokio::task::spawn_blocking(move || {
            poll_once(notify_enabled, shutdown_for_poll);
        })
        .await?;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Scheduled publishing
        tokio::task::spawn_blocking(schedule_tick).await?;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Auto-upgrade check (once per hour)
        if auto_upgrade {
            cycles_since_upgrade_check += 1;
            if cycles_since_upgrade_check >= upgrade_check_every {
                cycles_since_upgrade_check = 0;
                tokio::task::spawn_blocking(try_auto_upgrade).await?;
                // If we get here, exec() didn't happen (no upgrade or failed)
            }
        }

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Filter drift check (once per hour, best-effort)
        cycles_since_filter_check += 1;
        if cycles_since_filter_check >= filter_check_every {
            cycles_since_filter_check = 0;
            tokio::task::spawn_blocking(check_filter_drift).await?;
        }

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Sleep interruptibly — wake immediately on Ctrl-C
        tokio::select! {
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(interval)) => {}
            _ = shutdown_rx.changed() => { break; }
        }
    }

    println!("corky watch: stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::{AccountSyncState, LabelState};

    type AccountSpec<'a> = Vec<(&'a str, Vec<(&'a str, u32, u32)>)>;

    fn make_state(accounts: AccountSpec<'_>) -> SyncState {
        let mut state = SyncState::default();
        for (acct_name, labels) in accounts {
            let mut acct = AccountSyncState::default();
            for (label, uidvalidity, last_uid) in labels {
                acct.labels.insert(
                    label.to_string(),
                    LabelState {
                        uidvalidity,
                        last_uid,
                    },
                );
            }
            state.accounts.insert(acct_name.to_string(), acct);
        }
        state
    }

    #[test]
    fn snapshot_uids_empty_state() {
        let state = SyncState::default();
        let snap = snapshot_uids(&state);
        assert!(snap.is_empty());
    }

    #[test]
    fn snapshot_uids_captures_last_uid() {
        let state = make_state(vec![
            ("gmail", vec![("INBOX", 1, 100), ("Sent", 1, 50)]),
            ("proton", vec![("INBOX", 2, 200)]),
        ]);
        let snap = snapshot_uids(&state);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap["gmail"]["INBOX"], 100);
        assert_eq!(snap["gmail"]["Sent"], 50);
        assert_eq!(snap["proton"]["INBOX"], 200);
    }

    #[test]
    fn count_new_messages_no_change() {
        let snap = snapshot_uids(&make_state(vec![("gmail", vec![("INBOX", 1, 100)])]));
        assert_eq!(count_new_messages(&snap, &snap), 0);
    }

    #[test]
    fn count_new_messages_one_label_increased() {
        let before = snapshot_uids(&make_state(vec![(
            "gmail",
            vec![("INBOX", 1, 100), ("Sent", 1, 50)],
        )]));
        let after = snapshot_uids(&make_state(vec![(
            "gmail",
            vec![("INBOX", 1, 105), ("Sent", 1, 50)],
        )]));
        assert_eq!(count_new_messages(&before, &after), 1);
    }

    #[test]
    fn count_new_messages_multiple_labels_increased() {
        let before = snapshot_uids(&make_state(vec![
            ("gmail", vec![("INBOX", 1, 100)]),
            ("proton", vec![("INBOX", 2, 200)]),
        ]));
        let after = snapshot_uids(&make_state(vec![
            ("gmail", vec![("INBOX", 1, 110)]),
            ("proton", vec![("INBOX", 2, 210)]),
        ]));
        assert_eq!(count_new_messages(&before, &after), 2);
    }

    #[test]
    fn count_new_messages_new_account_in_after() {
        let before = snapshot_uids(&make_state(vec![("gmail", vec![("INBOX", 1, 100)])]));
        let after = snapshot_uids(&make_state(vec![
            ("gmail", vec![("INBOX", 1, 100)]),
            ("proton", vec![("INBOX", 2, 50)]),
        ]));
        // New account with uid > 0 counts as new
        assert_eq!(count_new_messages(&before, &after), 1);
    }

    #[test]
    fn count_new_messages_new_label_in_after() {
        let before = snapshot_uids(&make_state(vec![("gmail", vec![("INBOX", 1, 100)])]));
        let after = snapshot_uids(&make_state(vec![(
            "gmail",
            vec![("INBOX", 1, 100), ("Sent", 1, 30)],
        )]));
        // New label with uid > 0 counts as new
        assert_eq!(count_new_messages(&before, &after), 1);
    }

    #[test]
    fn count_new_messages_uid_decreased() {
        // UIDVALIDITY changed — uid went down. Should NOT count as new.
        let before = snapshot_uids(&make_state(vec![("gmail", vec![("INBOX", 1, 100)])]));
        let after = snapshot_uids(&make_state(vec![("gmail", vec![("INBOX", 2, 5)])]));
        assert_eq!(count_new_messages(&before, &after), 0);
    }
}
