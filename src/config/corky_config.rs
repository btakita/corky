//! Unified config type — parse .corky.toml (accounts + routing + mailboxes).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::accounts::{Account, OwnerConfig, WatchConfig};
use crate::config::contact::Contact;
use crate::config::topic::TopicConfig;
use crate::resolve;
use crate::social::profiles::Profile;
use crate::sync::imports::ImportConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CorkyConfig {
    #[serde(default)]
    pub owner: Option<OwnerConfig>,
    #[serde(default)]
    pub accounts: HashMap<String, Account>,
    #[serde(default)]
    pub contacts: HashMap<String, Contact>,
    #[serde(default)]
    pub routing: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub mailboxes: HashMap<String, MailboxConfig>,
    #[serde(default)]
    pub watch: Option<WatchConfig>,
    #[serde(default)]
    pub gmail: Option<GmailConfig>,
    #[serde(default)]
    pub linkedin: Option<OAuthClientConfig>,
    #[serde(default)]
    pub youtube: Option<OAuthClientConfig>,
    #[serde(default)]
    pub topics: HashMap<String, TopicConfig>,
    #[serde(default)]
    pub transcription: Option<TranscriptionConfig>,
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
    #[serde(default)]
    pub imports: Vec<ImportConfig>,
}

/// Gmail API config + filter rules (lives in .corky.toml under [gmail]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GmailConfig {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_id_cmd: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub client_secret_cmd: String,
    #[serde(default)]
    pub filters: Vec<GmailFilter>,
}

/// A Gmail filter rule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailFilter {
    pub label: Option<String>,
    #[serde(rename = "match", default)]
    pub match_fields: Vec<String>,
    #[serde(default)]
    pub addresses: Vec<String>,
    pub forward_to: Option<String>,
    #[serde(default)]
    pub star: bool,
    #[serde(default)]
    pub never_spam: bool,
    #[serde(default)]
    pub always_important: bool,
}

/// OAuth client credentials for a platform.
///
/// Resolution order per field: inline value > `_cmd` (shell command) > env var.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OAuthClientConfig {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_id_cmd: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub client_secret_cmd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MailboxConfig {
    #[serde(default)]
    pub auto_send: bool,
    #[serde(default)]
    pub permissions: HashMap<String, MailboxPermissions>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MailboxPermissions {
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub sync: bool,
    #[serde(default)]
    pub send: bool,
}

/// Transcription config for `corky transcribe`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TranscriptionConfig {
    /// Whisper model name: "tiny", "base", "small", "medium", "large-v3", "large-v3-turbo"
    #[serde(default = "default_model")]
    pub model: String,
    /// Directory to cache downloaded models
    #[serde(default)]
    pub model_path: String,
    /// Language code (e.g. "en"). Empty = auto-detect.
    #[serde(default)]
    pub language: String,
}

fn default_model() -> String {
    "large-v3-turbo".to_string()
}

/// Load .corky.toml (or corky.toml) from a given path or resolved location.
pub fn load_config(path: Option<&Path>) -> Result<CorkyConfig> {
    let path = path
        .map(PathBuf::from)
        .unwrap_or_else(resolve::corky_toml);
    if !path.exists() {
        bail!(
            ".corky.toml not found at {}.\nRun 'corky init' to create it.",
            path.display()
        );
    }
    let content = std::fs::read_to_string(&path)?;
    let config: CorkyConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Try loading config, returning None if the file doesn't exist.
pub fn try_load_config(path: Option<&Path>) -> Option<CorkyConfig> {
    let path = path
        .map(PathBuf::from)
        .unwrap_or_else(resolve::corky_toml);
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&content).ok()
}
