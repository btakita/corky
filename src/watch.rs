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

#[derive(Debug, Default, PartialEq, Eq)]
struct SyncCursorSnapshot {
    imap_uids: HashMap<String, HashMap<String, u32>>,
    gmail_history_ids: HashMap<String, HashMap<String, u64>>,
}

/// A periodic phase in the watch loop (#ckywatchsm), replacing the loose
/// `cycles_since_*: u64` counters with a typed cadence + per-phase circuit breaker.
///
/// Normal cadence: due once every `every` cycles. On repeated failure the phase
/// enters exponential backoff (skipped for `2^(failures-1)` cycles, capped at
/// `MAX_BACKOFF_CYCLES`) instead of being retried every tick — so e.g. a phase
/// failing on a persistent OAuth error stops hammering the loop. A success resets
/// the breaker.
struct PeriodicPhase {
    name: &'static str,
    every: u64,
    cycles_since: u64,
    consecutive_failures: u32,
    backoff_remaining: u64,
}

/// Cap on backoff so a long-failing phase still retries roughly hourly-ish.
const MAX_BACKOFF_CYCLES: u64 = 64;
/// Cap the exponent so `1 << exp` never overflows / exceeds the backoff cap.
const MAX_BACKOFF_EXP: u32 = 6;

impl PeriodicPhase {
    fn new(name: &'static str, every: u64) -> Self {
        PeriodicPhase {
            name,
            every: every.max(1),
            cycles_since: 0,
            consecutive_failures: 0,
            backoff_remaining: 0,
        }
    }

    /// Advance one watch cycle; returns true if this phase should run now.
    /// While in backoff the phase is skipped (and the cadence counter is held)
    /// until the backoff window elapses.
    fn tick(&mut self) -> bool {
        if self.backoff_remaining > 0 {
            self.backoff_remaining -= 1;
            return false;
        }
        self.cycles_since += 1;
        if self.cycles_since >= self.every {
            self.cycles_since = 0;
            true
        } else {
            false
        }
    }

    /// Clear the breaker after a successful run.
    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.backoff_remaining = 0;
    }

    /// Record a failure and arm exponential backoff.
    fn record_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let exp = self.consecutive_failures.min(MAX_BACKOFF_EXP);
        // exp >= 1 here, so 1 << (exp - 1) is well defined.
        self.backoff_remaining = (1u64 << (exp - 1)).min(MAX_BACKOFF_CYCLES);
    }

    fn in_backoff(&self) -> bool {
        self.backoff_remaining > 0
    }
}

/// Snapshot provider-specific sync cursors from current sync state.
fn snapshot_cursors(state: &SyncState) -> SyncCursorSnapshot {
    let mut snap = SyncCursorSnapshot::default();
    for (acct_name, acct_state) in &state.accounts {
        if !acct_state.labels.is_empty() {
            let mut labels = HashMap::new();
            for (label, ls) in &acct_state.labels {
                labels.insert(label.clone(), ls.last_uid);
            }
            snap.imap_uids.insert(acct_name.clone(), labels);
        }

        if !acct_state.gmail_labels.is_empty() {
            let mut labels = HashMap::new();
            for (label, ls) in &acct_state.gmail_labels {
                if let Some(last_history_id) = ls.last_history_id {
                    labels.insert(label.clone(), last_history_id);
                }
            }
            if !labels.is_empty() {
                snap.gmail_history_ids.insert(acct_name.clone(), labels);
            }
        }
    }
    snap
}

fn count_increased_u32(
    before: &HashMap<String, HashMap<String, u32>>,
    after: &HashMap<String, HashMap<String, u32>>,
) -> usize {
    let mut count = 0;
    for (acct_name, labels) in after {
        let before_acct = before.get(acct_name);
        for (label, cursor) in labels {
            let before_cursor = before_acct.and_then(|a| a.get(label)).copied().unwrap_or(0);
            if *cursor > before_cursor {
                count += 1;
            }
        }
    }
    count
}

fn count_increased_u64(
    before: &HashMap<String, HashMap<String, u64>>,
    after: &HashMap<String, HashMap<String, u64>>,
) -> usize {
    let mut count = 0;
    for (acct_name, labels) in after {
        let before_acct = before.get(acct_name);
        for (label, cursor) in labels {
            let before_cursor = before_acct.and_then(|a| a.get(label)).copied().unwrap_or(0);
            if *cursor > before_cursor {
                count += 1;
            }
        }
    }
    count
}

/// Count labels where IMAP last_uid or Gmail API last_history_id increased.
fn count_new_messages(before: &SyncCursorSnapshot, after: &SyncCursorSnapshot) -> usize {
    count_increased_u32(&before.imap_uids, &after.imap_uids)
        + count_increased_u64(&before.gmail_history_ids, &after.gmail_history_ids)
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
    let before = snapshot_cursors(&state);

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

    let after = snapshot_cursors(&state);
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

/// Run one blocking watch-loop tick, isolating panics so a single bad tick can
/// never crash the daemon (#ckywatchpanic).
///
/// `spawn_blocking` catches a panic in the closure and surfaces it as a
/// `JoinError` (requires the unwind panic strategy — see `[profile.release]` in
/// Cargo.toml). The previous code propagated that `JoinError` with `?`, exiting
/// the loop on the first panicking tick and violating the never-crashes-loop
/// contract. Here we log it and let the loop continue. The tick's own return
/// value is intentionally discarded, matching the prior behavior.
/// Run a watch tick on a blocking thread, swallowing panics so one bad tick never
/// crashes the loop. Returns `true` if the tick completed, `false` if it panicked
/// or was cancelled — the caller feeds that into a phase circuit breaker.
async fn run_tick<F, R>(label: &str, f: F) -> bool
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(_) => true,
        Err(e) if e.is_panic() => {
            eprintln!("  warning: {label} tick panicked; continuing watch loop");
            false
        }
        Err(_) => {
            // Cancelled (e.g. runtime shutdown) — nothing to recover, just continue.
            eprintln!("  warning: {label} tick did not complete; continuing watch loop");
            false
        }
    }
}

/// Feed a tick outcome into a phase's circuit breaker, noting when it backs off.
fn update_phase_breaker(phase: &mut PeriodicPhase, ok: bool) {
    if ok {
        phase.record_success();
    } else {
        phase.record_failure();
        if phase.in_backoff() {
            eprintln!(
                "  note: {} tick failed {}x; backing off {} cycle(s)",
                phase.name, phase.consecutive_failures, phase.backoff_remaining
            );
        }
    }
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

    // Typed periodic phases (#ckywatchsm) — roughly once per hour, each with its
    // own circuit breaker so a persistently-failing phase backs off instead of
    // retrying every cycle.
    let check_every = (3600 / interval).max(1);
    let mut upgrade_phase = PeriodicPhase::new("auto-upgrade", check_every);
    let mut filter_phase = PeriodicPhase::new("filter-drift", check_every);

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Run sync in a blocking context
        let notify_enabled = config.notify;
        let shutdown_for_poll = shutdown.clone();
        run_tick("sync", move || {
            poll_once(notify_enabled, shutdown_for_poll);
        })
        .await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Scheduled publishing
        run_tick("schedule", schedule_tick).await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Auto-upgrade check (once per hour; circuit-broken on repeated failure)
        if auto_upgrade && upgrade_phase.tick() {
            let ok = run_tick("auto-upgrade", try_auto_upgrade).await;
            // If we get here, exec() didn't happen (no upgrade or failed).
            update_phase_breaker(&mut upgrade_phase, ok);
        }

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Filter drift check (once per hour, best-effort; circuit-broken on failure)
        if filter_phase.tick() {
            let ok = run_tick("filter-drift", check_filter_drift).await;
            update_phase_breaker(&mut filter_phase, ok);
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
    use crate::sync::types::{AccountSyncState, GmailLabelState, LabelState};

    #[test]
    fn periodic_phase_fires_on_cadence() {
        let mut p = PeriodicPhase::new("x", 3);
        // Due exactly every 3rd cycle.
        assert_eq!(
            (0..6).map(|_| p.tick()).collect::<Vec<_>>(),
            vec![false, false, true, false, false, true]
        );
    }

    #[test]
    fn periodic_phase_every_zero_is_clamped_to_one() {
        let mut p = PeriodicPhase::new("x", 0);
        assert!(p.tick());
        assert!(p.tick());
    }

    #[test]
    fn failure_arms_exponential_backoff_then_recovers() {
        let mut p = PeriodicPhase::new("x", 1); // due every cycle
        assert!(p.tick());
        // 1st failure → backoff 1 cycle (1 << 0).
        p.record_failure();
        assert_eq!(p.consecutive_failures, 1);
        assert!(p.in_backoff());
        assert!(!p.tick()); // consumes the 1-cycle backoff
        assert!(!p.in_backoff());
        assert!(p.tick()); // due again
        // 2nd consecutive failure → backoff 2 cycles (1 << 1).
        p.record_failure();
        assert_eq!(p.consecutive_failures, 2);
        assert_eq!(p.backoff_remaining, 2);
        assert!(!p.tick());
        assert!(!p.tick());
        assert!(p.tick());
        // Success clears the breaker.
        p.record_success();
        assert_eq!(p.consecutive_failures, 0);
        assert!(!p.in_backoff());
    }

    #[test]
    fn backoff_is_capped() {
        let mut p = PeriodicPhase::new("x", 1);
        for _ in 0..100 {
            p.record_failure();
        }
        assert!(p.backoff_remaining <= MAX_BACKOFF_CYCLES);
    }

    #[tokio::test]
    async fn run_tick_swallows_panic_and_continues() {
        // #ckywatchpanic: a panicking tick must NOT propagate — run_tick returns
        // normally so the watch loop keeps running. (Test builds use the unwind
        // panic strategy, matching the release profile after the fix.)
        run_tick("panic-tick", || panic!("boom")).await;
        // A normal tick with a non-unit return also completes; the value is
        // discarded just like the loop expects.
        run_tick("ok-tick", || 42_usize).await;
        // Reaching here means neither call propagated.
    }

    type AccountSpec<'a> = Vec<(&'a str, Vec<(&'a str, u32, u32)>)>;
    type GmailApiAccountSpec<'a> = Vec<(&'a str, Vec<(&'a str, Option<u64>)>)>;

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

    fn make_gmail_api_state(accounts: GmailApiAccountSpec<'_>) -> SyncState {
        let mut state = SyncState::default();
        for (acct_name, labels) in accounts {
            let mut acct = AccountSyncState::default();
            for (label, last_history_id) in labels {
                acct.gmail_labels
                    .insert(label.to_string(), GmailLabelState { last_history_id });
            }
            state.accounts.insert(acct_name.to_string(), acct);
        }
        state
    }

    #[test]
    fn snapshot_cursors_empty_state() {
        let state = SyncState::default();
        let snap = snapshot_cursors(&state);
        assert!(snap.imap_uids.is_empty());
        assert!(snap.gmail_history_ids.is_empty());
    }

    #[test]
    fn snapshot_cursors_captures_last_uid() {
        let state = make_state(vec![
            ("gmail", vec![("INBOX", 1, 100), ("Sent", 1, 50)]),
            ("proton", vec![("INBOX", 2, 200)]),
        ]);
        let snap = snapshot_cursors(&state);
        assert_eq!(snap.imap_uids.len(), 2);
        assert_eq!(snap.imap_uids["gmail"]["INBOX"], 100);
        assert_eq!(snap.imap_uids["gmail"]["Sent"], 50);
        assert_eq!(snap.imap_uids["proton"]["INBOX"], 200);
    }

    #[test]
    fn snapshot_cursors_captures_gmail_history_id() {
        let state = make_gmail_api_state(vec![(
            "gmail-api",
            vec![("INBOX", Some(1234)), ("Sent", None)],
        )]);
        let snap = snapshot_cursors(&state);
        assert!(snap.imap_uids.is_empty());
        assert_eq!(snap.gmail_history_ids.len(), 1);
        assert_eq!(snap.gmail_history_ids["gmail-api"]["INBOX"], 1234);
        assert!(!snap.gmail_history_ids["gmail-api"].contains_key("Sent"));
    }

    #[test]
    fn count_new_messages_no_change() {
        let snap = snapshot_cursors(&make_state(vec![("gmail", vec![("INBOX", 1, 100)])]));
        assert_eq!(count_new_messages(&snap, &snap), 0);
    }

    #[test]
    fn count_new_messages_one_label_increased() {
        let before = snapshot_cursors(&make_state(vec![(
            "gmail",
            vec![("INBOX", 1, 100), ("Sent", 1, 50)],
        )]));
        let after = snapshot_cursors(&make_state(vec![(
            "gmail",
            vec![("INBOX", 1, 105), ("Sent", 1, 50)],
        )]));
        assert_eq!(count_new_messages(&before, &after), 1);
    }

    #[test]
    fn count_new_messages_multiple_labels_increased() {
        let before = snapshot_cursors(&make_state(vec![
            ("gmail", vec![("INBOX", 1, 100)]),
            ("proton", vec![("INBOX", 2, 200)]),
        ]));
        let after = snapshot_cursors(&make_state(vec![
            ("gmail", vec![("INBOX", 1, 110)]),
            ("proton", vec![("INBOX", 2, 210)]),
        ]));
        assert_eq!(count_new_messages(&before, &after), 2);
    }

    #[test]
    fn count_new_messages_new_account_in_after() {
        let before = snapshot_cursors(&make_state(vec![("gmail", vec![("INBOX", 1, 100)])]));
        let after = snapshot_cursors(&make_state(vec![
            ("gmail", vec![("INBOX", 1, 100)]),
            ("proton", vec![("INBOX", 2, 50)]),
        ]));
        // New account with uid > 0 counts as new
        assert_eq!(count_new_messages(&before, &after), 1);
    }

    #[test]
    fn count_new_messages_new_label_in_after() {
        let before = snapshot_cursors(&make_state(vec![("gmail", vec![("INBOX", 1, 100)])]));
        let after = snapshot_cursors(&make_state(vec![(
            "gmail",
            vec![("INBOX", 1, 100), ("Sent", 1, 30)],
        )]));
        // New label with uid > 0 counts as new
        assert_eq!(count_new_messages(&before, &after), 1);
    }

    #[test]
    fn count_new_messages_uid_decreased() {
        // UIDVALIDITY changed — uid went down. Should NOT count as new.
        let before = snapshot_cursors(&make_state(vec![("gmail", vec![("INBOX", 1, 100)])]));
        let after = snapshot_cursors(&make_state(vec![("gmail", vec![("INBOX", 2, 5)])]));
        assert_eq!(count_new_messages(&before, &after), 0);
    }

    #[test]
    fn count_new_messages_gmail_history_id_increased() {
        let before = snapshot_cursors(&make_gmail_api_state(vec![(
            "gmail-api",
            vec![("INBOX", Some(1000)), ("Sent", Some(500))],
        )]));
        let after = snapshot_cursors(&make_gmail_api_state(vec![(
            "gmail-api",
            vec![("INBOX", Some(1001)), ("Sent", Some(500))],
        )]));
        assert_eq!(count_new_messages(&before, &after), 1);
    }

    #[test]
    fn count_new_messages_gmail_history_id_decreased() {
        let before = snapshot_cursors(&make_gmail_api_state(vec![(
            "gmail-api",
            vec![("INBOX", Some(1000))],
        )]));
        let after = snapshot_cursors(&make_gmail_api_state(vec![(
            "gmail-api",
            vec![("INBOX", Some(999))],
        )]));
        assert_eq!(count_new_messages(&before, &after), 0);
    }

    #[test]
    fn count_new_messages_gmail_history_id_new_label() {
        let before = snapshot_cursors(&make_gmail_api_state(vec![(
            "gmail-api",
            vec![("INBOX", Some(1000))],
        )]));
        let after = snapshot_cursors(&make_gmail_api_state(vec![(
            "gmail-api",
            vec![("INBOX", Some(1000)), ("Updates", Some(42))],
        )]));
        assert_eq!(count_new_messages(&before, &after), 1);
    }
}
