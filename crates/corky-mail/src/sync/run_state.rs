//! Per-account sync run-state machine (#ckysyncsm).
//!
//! Layers a typed run *phase* on top of the cursor data in [`SyncState`], persisted
//! as a sidecar (`.sync-run-state.json`) so a crashed `corky sync` or watch tick can
//! detect an interrupted run (a non-terminal phase left on disk) and resume/repair
//! instead of silently re-fetching from the cursor. The cursor data in `SyncState`
//! stays the source of truth for *what* was fetched; this records *where in the run*
//! a crash happened.
//!
//! [`SyncState`]: super::types::SyncState

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::file_store;
use crate::resolve;

/// Phase of a single account's sync run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncRunState {
    /// No run in progress (or cleanly reset).
    #[default]
    Idle,
    /// Pulling messages from the provider (IMAP/Gmail API).
    Fetching,
    /// Merging fetched messages into existing conversation files.
    Merging,
    /// Writing conversation files / state to disk.
    Writing,
    /// Run finished successfully (terminal).
    Done,
    /// Run failed (terminal); `error` carries the message.
    Error,
}

/// Events driving the [`SyncRunState`] machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRunEvent {
    /// Begin a run.
    Start,
    /// Fetch phase finished.
    Fetched,
    /// Merge phase finished.
    Merged,
    /// The whole run finished successfully (from any in-flight phase).
    Complete,
    /// The run failed.
    Fail,
    /// Clear back to Idle.
    Reset,
}

impl SyncRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncRunState::Idle => "idle",
            SyncRunState::Fetching => "fetching",
            SyncRunState::Merging => "merging",
            SyncRunState::Writing => "writing",
            SyncRunState::Done => "done",
            SyncRunState::Error => "error",
        }
    }

    /// Terminal phases — a completed run, not a resume candidate.
    pub fn is_terminal(self) -> bool {
        matches!(self, SyncRunState::Done | SyncRunState::Error)
    }

    /// An in-flight phase: a run left here on disk was interrupted (crash) and
    /// should be resumed/repaired by the next sync rather than blindly re-fetched.
    pub fn is_in_flight(self) -> bool {
        matches!(
            self,
            SyncRunState::Fetching | SyncRunState::Merging | SyncRunState::Writing
        )
    }

    /// Advance for a [`SyncRunEvent`], rejecting invalid edges.
    pub fn transition(self, event: SyncRunEvent) -> Result<SyncRunState> {
        use SyncRunState::*;
        let next = match (self, event) {
            (Idle | Done | Error, SyncRunEvent::Start) => Fetching,
            (Fetching, SyncRunEvent::Fetched) => Merging,
            (Merging, SyncRunEvent::Merged) => Writing,
            (Fetching | Merging | Writing, SyncRunEvent::Complete) => Done,
            (Fetching | Merging | Writing, SyncRunEvent::Fail) => Error,
            (_, SyncRunEvent::Reset) => Idle,
            (from, ev) => bail!("Invalid sync run transition: {from} -> {ev:?}"),
        };
        Ok(next)
    }
}

impl std::fmt::Display for SyncRunState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Persisted per-account run record (the resume marker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRunRecord {
    pub phase: SyncRunState,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The `.sync-run-state.json` sidecar: per-account run records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRunLog {
    #[serde(default)]
    pub accounts: HashMap<String, SyncRunRecord>,
}

fn run_state_path() -> PathBuf {
    resolve::data_dir().join(".sync-run-state.json")
}

/// Pure transition applied to a log (no IO) — the testable core of [`record`].
pub fn apply_event(
    log: &mut SyncRunLog,
    account: &str,
    event: SyncRunEvent,
    now: DateTime<Utc>,
    error: Option<String>,
) -> Result<SyncRunState> {
    let current = log.accounts.get(account).map(|r| r.phase).unwrap_or_default();
    let next = current.transition(event)?;
    // `started_at` is stamped when a fresh run begins; otherwise carried forward.
    let started_at = if event == SyncRunEvent::Start {
        now
    } else {
        log.accounts
            .get(account)
            .map(|r| r.started_at)
            .unwrap_or(now)
    };
    log.accounts.insert(
        account.to_string(),
        SyncRunRecord {
            phase: next,
            started_at,
            updated_at: now,
            error,
        },
    );
    Ok(next)
}

/// Load the persisted run log (empty if the sidecar does not exist).
pub fn load() -> Result<SyncRunLog> {
    file_store::load_json_or_default(&run_state_path())
}

/// Record a run-phase transition for `account`, persisted via the locked atomic
/// store so concurrent account writers don't clobber each other.
pub fn record(account: &str, event: SyncRunEvent, error: Option<String>) -> Result<SyncRunState> {
    let now = Utc::now();
    let mut result = SyncRunState::Idle;
    file_store::save_json_with_lock::<SyncRunLog, _>(&run_state_path(), None, |mut current| {
        result = apply_event(&mut current, account, event, now, error.clone())?;
        Ok(current)
    })?;
    Ok(result)
}

/// Accounts whose last recorded run is still in-flight (interrupted) — the resume
/// candidates a later sync/watch tick should reconcile. Sorted for determinism.
pub fn interrupted_accounts(log: &SyncRunLog) -> Vec<String> {
    let mut names: Vec<String> = log
        .accounts
        .iter()
        .filter(|(_, r)| r.phase.is_in_flight())
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn happy_path_transitions() {
        use SyncRunState::*;
        assert_eq!(Idle.transition(SyncRunEvent::Start).unwrap(), Fetching);
        assert_eq!(Fetching.transition(SyncRunEvent::Fetched).unwrap(), Merging);
        assert_eq!(Merging.transition(SyncRunEvent::Merged).unwrap(), Writing);
        assert_eq!(Writing.transition(SyncRunEvent::Complete).unwrap(), Done);
        // Complete is reachable from any in-flight phase (boundary-level wiring).
        assert_eq!(Fetching.transition(SyncRunEvent::Complete).unwrap(), Done);
        // A new run after Done/Error starts over.
        assert_eq!(Done.transition(SyncRunEvent::Start).unwrap(), Fetching);
        assert_eq!(Error.transition(SyncRunEvent::Start).unwrap(), Fetching);
    }

    #[test]
    fn failure_and_reset() {
        use SyncRunState::*;
        assert_eq!(Merging.transition(SyncRunEvent::Fail).unwrap(), Error);
        assert_eq!(Writing.transition(SyncRunEvent::Reset).unwrap(), Idle);
        // Invalid edges rejected.
        assert!(Idle.transition(SyncRunEvent::Fetched).is_err());
        assert!(Done.transition(SyncRunEvent::Complete).is_err());
        assert!(Idle.transition(SyncRunEvent::Fail).is_err());
    }

    #[test]
    fn terminal_and_in_flight_predicates() {
        assert!(SyncRunState::Done.is_terminal());
        assert!(SyncRunState::Error.is_terminal());
        assert!(!SyncRunState::Fetching.is_terminal());
        assert!(SyncRunState::Fetching.is_in_flight());
        assert!(SyncRunState::Writing.is_in_flight());
        assert!(!SyncRunState::Idle.is_in_flight());
        assert!(!SyncRunState::Done.is_in_flight());
    }

    #[test]
    fn apply_event_stamps_started_at_and_carries_forward() {
        let mut log = SyncRunLog::default();
        let t = t0();
        apply_event(&mut log, "work", SyncRunEvent::Start, t, None).unwrap();
        let started = log.accounts["work"].started_at;
        assert_eq!(log.accounts["work"].phase, SyncRunState::Fetching);
        // A later event keeps started_at, advances updated_at.
        let t1 = t + chrono::Duration::seconds(5);
        apply_event(&mut log, "work", SyncRunEvent::Fetched, t1, None).unwrap();
        assert_eq!(log.accounts["work"].phase, SyncRunState::Merging);
        assert_eq!(log.accounts["work"].started_at, started);
        assert_eq!(log.accounts["work"].updated_at, t1);
    }

    #[test]
    fn interrupted_accounts_lists_in_flight_only() {
        let mut log = SyncRunLog::default();
        let t = t0();
        // a: interrupted mid-fetch; b: completed; c: failed.
        apply_event(&mut log, "a", SyncRunEvent::Start, t, None).unwrap();
        apply_event(&mut log, "b", SyncRunEvent::Start, t, None).unwrap();
        apply_event(&mut log, "b", SyncRunEvent::Complete, t, None).unwrap();
        apply_event(&mut log, "c", SyncRunEvent::Start, t, None).unwrap();
        apply_event(&mut log, "c", SyncRunEvent::Fail, t, Some("boom".into())).unwrap();
        assert_eq!(interrupted_accounts(&log), vec!["a".to_string()]);
        assert_eq!(log.accounts["c"].error.as_deref(), Some("boom"));
    }

    #[test]
    fn log_serde_roundtrips() {
        let mut log = SyncRunLog::default();
        apply_event(&mut log, "work", SyncRunEvent::Start, t0(), None).unwrap();
        let json = serde_json::to_string(&log).unwrap();
        assert!(json.contains("\"fetching\""), "got: {json}");
        let back: SyncRunLog = serde_json::from_str(&json).unwrap();
        assert_eq!(back, log);
    }
}
