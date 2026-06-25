//! Sync data types: Message, Thread, SyncState.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub cc: String,
    pub date: String,
    pub subject: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub subject: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub last_date: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracking: Vec<String>,
    /// Additional provider thread keys that map to this thread (#ckythreadmerge).
    /// The primary identity is `id`; `aliases` lets a thread written under one
    /// provider's key (e.g. Gmail `threadId`) be found when another provider
    /// (e.g. IMAP subject-key) or a refetch resolves by a different key. A file
    /// "claims" a key K when `id == K` or `aliases` contains K.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelState {
    pub uidvalidity: u32,
    pub last_uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GmailLabelState {
    /// Highest historyId seen for this label (for incremental sync).
    #[serde(default)]
    pub last_history_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AccountSyncState {
    #[serde(default)]
    pub labels: HashMap<String, LabelState>,
    /// Gmail API sync state (per-label). Only used when provider = "gmail-api".
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub gmail_labels: HashMap<String, GmailLabelState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContactSyncState {
    /// Per-mailbox: FNV-1a hash of the CLAUDE.md content at last sync.
    /// Key = mailbox name, Value = hash hex string.
    #[serde(default)]
    pub mailboxes: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SyncState {
    #[serde(default)]
    pub accounts: HashMap<String, AccountSyncState>,
    #[serde(default)]
    pub contacts: HashMap<String, ContactSyncState>,
}

pub fn load_state(data: &[u8]) -> anyhow::Result<SyncState> {
    let state: SyncState = serde_json::from_slice(data)?;
    Ok(state)
}
