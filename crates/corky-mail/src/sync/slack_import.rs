//! Parse Slack workspace export ZIPs and convert to corky conversations.

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use super::imap_sync::merge_message_to_file;
use super::types::Message;

// ---------------------------------------------------------------------------
// Slack export JSON types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SlackUser {
    pub id: String,
    #[serde(default)]
    pub real_name: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub profile: Option<SlackUserProfile>,
}

#[derive(Debug, Deserialize)]
pub struct SlackUserProfile {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SlackChannel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlackMessage {
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub thread_ts: Option<String>,
    #[serde(default, rename = "type")]
    pub msg_type: Option<String>,
    #[serde(default)]
    pub subtype: Option<String>,
}

// ---------------------------------------------------------------------------
// Slack mrkdwn resolution regexes
// ---------------------------------------------------------------------------

/// `<@U1234>` → `@DisplayName`
static USER_MENTION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<@(U[A-Z0-9]+)>").unwrap());

/// `<#C1234|name>` → `#name`
static CHANNEL_MENTION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<#C[A-Z0-9]+\|([^>]+)>").unwrap());

/// `<url|text>` → `text (url)`
static LINK_WITH_TEXT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<(https?://[^|>]+)\|([^>]+)>").unwrap());

/// `<url>` → `url`
static BARE_LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<(https?://[^>]+)>").unwrap());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Import a Slack workspace export ZIP into corky conversations.
///
/// - `zip_path`: path to the export `.zip` file
/// - `label`: label to assign to imported threads (default: `"slack"`)
/// - `out_dir`: directory to write conversation markdown files
/// - `account_name`: account name for metadata
pub fn run(zip_path: &Path, label: &str, out_dir: &Path, account_name: &str) -> Result<()> {
    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("Cannot open ZIP: {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("Invalid ZIP: {}", zip_path.display()))?;

    // 1. Parse users.json → HashMap<user_id, display_name>
    let users = parse_users(&mut archive)?;

    // 2. Parse channels.json → HashMap<channel_name, SlackChannel>
    let channels = parse_channels(&mut archive)?;

    // Build reverse map: channel_name → channel_id
    let channel_id_by_name: HashMap<&str, &str> = channels
        .iter()
        .map(|(name, ch)| (name.as_str(), ch.id.as_str()))
        .collect();

    // 3. Discover channel directories from file paths
    let channel_dirs = discover_channel_dirs(&mut archive, &channels);

    println!(
        "Slack import: {} users, {} channels",
        users.len(),
        channel_dirs.len()
    );

    // 4. For each channel, parse date JSON files and group by thread
    for channel_name in &channel_dirs {
        let channel_id = channel_id_by_name
            .get(channel_name.as_str())
            .copied()
            .unwrap_or(channel_name.as_str());

        let messages = collect_channel_messages(&mut archive, channel_name)?;
        if messages.is_empty() {
            continue;
        }

        println!("  #{}: {} messages", channel_name, messages.len());

        // Group by thread_ts (or own ts for standalone messages)
        let threads = group_into_threads(&messages);

        for (thread_ts, thread_msgs) in &threads {
            let thread_key = format!("slack:{}:{}", channel_id, thread_ts);

            // Subject: first 60 chars of parent message text
            let parent_text = &thread_msgs[0].text;
            let subject = derive_subject(parent_text, channel_name, thread_ts);

            let label_name = format!("{}:{}", label, channel_name);

            for msg in thread_msgs {
                let from = resolve_user_name(&users, msg.user.as_deref().unwrap_or("unknown"));
                let date = ts_to_rfc2822(&msg.ts);
                let body = resolve_mrkdwn(&msg.text, &users);

                let message = Message {
                    id: msg.ts.clone(),
                    thread_id: thread_key.clone(),
                    from,
                    to: String::new(),
                    cc: String::new(),
                    date,
                    subject: subject.clone(),
                    body,
                    message_id: None,
                };

                merge_message_to_file(out_dir, &label_name, account_name, &message, &thread_key)?;
            }
        }
    }

    println!("Slack import complete.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse `users.json` from the archive into a user ID → display name map.
fn parse_users(archive: &mut zip::ZipArchive<std::fs::File>) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let mut file = match archive.by_name("users.json") {
        Ok(f) => f,
        Err(_) => return Ok(map), // no users.json — IDs won't resolve but import continues
    };
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    let users: Vec<SlackUser> = serde_json::from_str(&buf)?;
    for u in users {
        let display = u
            .profile
            .as_ref()
            .and_then(|p| p.display_name.as_ref())
            .filter(|n| !n.is_empty())
            .cloned()
            .or(u.real_name.filter(|n| !n.is_empty()))
            .unwrap_or(u.name);
        map.insert(u.id, display);
    }
    Ok(map)
}

/// Parse `channels.json` from the archive.
fn parse_channels(
    archive: &mut zip::ZipArchive<std::fs::File>,
) -> Result<HashMap<String, SlackChannel>> {
    let mut map = HashMap::new();
    let mut file = match archive.by_name("channels.json") {
        Ok(f) => f,
        Err(_) => return Ok(map),
    };
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    let channels: Vec<SlackChannel> = serde_json::from_str(&buf)?;
    for ch in channels {
        map.insert(ch.name.clone(), ch);
    }
    Ok(map)
}

/// Discover channel directory names by scanning ZIP entries.
fn discover_channel_dirs(
    archive: &mut zip::ZipArchive<std::fs::File>,
    channels: &HashMap<String, SlackChannel>,
) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // First use known channel names from channels.json
    for name in channels.keys() {
        if seen.insert(name.clone()) {
            dirs.push(name.clone());
        }
    }

    // Also discover from ZIP paths: entries like "general/2024-01-15.json"
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index_raw(i) {
            let name = entry.name().to_string();
            if let Some(slash_pos) = name.find('/') {
                let dir = &name[..slash_pos];
                // Skip known non-channel files at root
                if dir != "users"
                    && dir != "channels"
                    && !dir.ends_with(".json")
                    && seen.insert(dir.to_string())
                {
                    dirs.push(dir.to_string());
                }
            }
        }
    }

    dirs.sort();
    dirs
}

/// Collect all messages from a channel directory in the archive.
fn collect_channel_messages(
    archive: &mut zip::ZipArchive<std::fs::File>,
    channel_name: &str,
) -> Result<Vec<SlackMessage>> {
    let mut all_msgs = Vec::new();

    // Collect matching file names first (can't borrow archive mutably while iterating)
    let file_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let entry = archive.by_index_raw(i).ok()?;
            let name = entry.name().to_string();
            if name.starts_with(&format!("{}/", channel_name)) && name.ends_with(".json") {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    for file_name in file_names {
        let mut file = archive.by_name(&file_name)?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        let msgs: Vec<SlackMessage> = serde_json::from_str(&buf).unwrap_or_default();
        for msg in msgs {
            // Skip non-message entries (join/leave/etc)
            if msg.subtype.is_some() {
                continue;
            }
            all_msgs.push(msg);
        }
    }

    // Sort by timestamp
    all_msgs.sort_by(|a, b| a.ts.cmp(&b.ts));
    Ok(all_msgs)
}

/// Group messages by thread_ts. Standalone messages (no thread_ts) use their own ts.
fn group_into_threads(messages: &[SlackMessage]) -> Vec<(String, Vec<&SlackMessage>)> {
    let mut map: HashMap<String, Vec<&SlackMessage>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for msg in messages {
        let key = msg.thread_ts.as_deref().unwrap_or(&msg.ts).to_string();
        if !map.contains_key(&key) {
            order.push(key.clone());
        }
        map.entry(key).or_default().push(msg);
    }

    order
        .into_iter()
        .filter_map(|key| {
            let msgs = map.remove(&key)?;
            Some((key, msgs))
        })
        .collect()
}

/// Derive a subject from the parent message text.
/// Falls back to `#channel_name -- date` if text is empty.
fn derive_subject(parent_text: &str, channel_name: &str, thread_ts: &str) -> String {
    let first_line = parent_text.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        let date = ts_to_date(thread_ts);
        format!("#{} -- {}", channel_name, date)
    } else if first_line.len() > 60 {
        // Don't split in the middle of a multi-byte char
        let mut end = 60;
        while !first_line.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        first_line[..end].to_string()
    } else {
        first_line.to_string()
    }
}

/// Resolve a Slack user ID to a display name.
fn resolve_user_name(users: &HashMap<String, String>, user_id: &str) -> String {
    users
        .get(user_id)
        .cloned()
        .unwrap_or_else(|| user_id.to_string())
}

/// Convert a Slack `ts` (epoch seconds with microsecond fraction) to RFC 2822.
fn ts_to_rfc2822(ts: &str) -> String {
    let secs: f64 = ts.parse().unwrap_or(0.0);
    let dt = chrono::DateTime::from_timestamp(secs as i64, 0).unwrap_or_default();
    dt.to_rfc2822()
}

/// Convert a Slack `ts` to a date string (YYYY-MM-DD).
fn ts_to_date(ts: &str) -> String {
    let secs: f64 = ts.parse().unwrap_or(0.0);
    let dt = chrono::DateTime::from_timestamp(secs as i64, 0).unwrap_or_default();
    dt.format("%Y-%m-%d").to_string()
}

/// Resolve Slack mrkdwn to plain text.
///
/// Handles: `<@U1234>` → `@DisplayName`, `<#C1234|name>` → `#name`,
/// `<url|text>` → `text (url)`, `<url>` → `url`.
fn resolve_mrkdwn(text: &str, users: &HashMap<String, String>) -> String {
    // Resolve user mentions
    let result = USER_MENTION_RE.replace_all(text, |caps: &regex::Captures| {
        let uid = &caps[1];
        let name = users.get(uid).map(|n| n.as_str()).unwrap_or(uid);
        format!("@{}", name)
    });

    // Resolve channel mentions
    let result = CHANNEL_MENTION_RE.replace_all(&result, "#$1");

    // Resolve links with text
    let result = LINK_WITH_TEXT_RE.replace_all(&result, "$2 ($1)");

    // Resolve bare links
    let result = BARE_LINK_RE.replace_all(&result, "$1");

    result.into_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_resolve_mrkdwn_user_mention() {
        let mut users = HashMap::new();
        users.insert("U123ABC".to_string(), "Alice".to_string());
        assert_eq!(resolve_mrkdwn("Hello <@U123ABC>!", &users), "Hello @Alice!");
    }

    #[test]
    fn test_resolve_mrkdwn_unknown_user() {
        let users = HashMap::new();
        assert_eq!(
            resolve_mrkdwn("Hello <@U999ZZZ>!", &users),
            "Hello @U999ZZZ!"
        );
    }

    #[test]
    fn test_resolve_mrkdwn_channel() {
        let users = HashMap::new();
        assert_eq!(
            resolve_mrkdwn("See <#C123|general>", &users),
            "See #general"
        );
    }

    #[test]
    fn test_resolve_mrkdwn_link_with_text() {
        let users = HashMap::new();
        assert_eq!(
            resolve_mrkdwn("Check <https://example.com|this out>", &users),
            "Check this out (https://example.com)"
        );
    }

    #[test]
    fn test_resolve_mrkdwn_bare_link() {
        let users = HashMap::new();
        assert_eq!(
            resolve_mrkdwn("Visit <https://example.com>", &users),
            "Visit https://example.com"
        );
    }

    #[test]
    fn test_ts_to_rfc2822() {
        let rfc = ts_to_rfc2822("1705312800.000000");
        assert!(rfc.contains("2024"));
    }

    #[test]
    fn test_derive_subject_from_text() {
        assert_eq!(
            derive_subject("Hello world", "general", "1705312800.000000"),
            "Hello world"
        );
    }

    #[test]
    fn test_derive_subject_truncated() {
        let long_text = "a".repeat(100);
        let subject = derive_subject(&long_text, "general", "1705312800.000000");
        assert_eq!(subject.len(), 60);
    }

    #[test]
    fn test_derive_subject_fallback() {
        let subject = derive_subject("", "general", "1705312800.000000");
        assert!(subject.starts_with("#general -- "));
    }

    #[test]
    fn test_group_into_threads() {
        let msgs = vec![
            SlackMessage {
                user: Some("U1".into()),
                text: "parent".into(),
                ts: "100.0".into(),
                thread_ts: None,
                msg_type: None,
                subtype: None,
            },
            SlackMessage {
                user: Some("U2".into()),
                text: "reply".into(),
                ts: "101.0".into(),
                thread_ts: Some("100.0".into()),
                msg_type: None,
                subtype: None,
            },
            SlackMessage {
                user: Some("U3".into()),
                text: "standalone".into(),
                ts: "200.0".into(),
                thread_ts: None,
                msg_type: None,
                subtype: None,
            },
        ];
        let threads = group_into_threads(&msgs);
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].0, "100.0");
        assert_eq!(threads[0].1.len(), 2); // parent + reply
        assert_eq!(threads[1].0, "200.0");
        assert_eq!(threads[1].1.len(), 1); // standalone
    }

    #[test]
    fn test_full_import_roundtrip() {
        // Build a minimal Slack export ZIP in memory, then import it
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("export.zip");
        let out_dir = dir.path().join("conversations");

        // Build ZIP
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            // users.json
            zip.start_file("users.json", options).unwrap();
            zip.write_all(
                br#"[{"id":"U1","name":"alice","real_name":"Alice Smith","profile":{"display_name":"Alice"}}]"#,
            )
            .unwrap();

            // channels.json
            zip.start_file("channels.json", options).unwrap();
            zip.write_all(br#"[{"id":"C1","name":"general"}]"#).unwrap();

            // general/2024-01-15.json
            zip.start_file("general/2024-01-15.json", options).unwrap();
            zip.write_all(
                br#"[
                    {"user":"U1","text":"Hello everyone!","ts":"1705312800.000000","type":"message"},
                    {"user":"U1","text":"Reply in thread","ts":"1705312900.000000","thread_ts":"1705312800.000000","type":"message"}
                ]"#,
            )
            .unwrap();

            zip.finish().unwrap();
        }

        // Run import
        run(&zip_path, "slack", &out_dir, "slack-work").unwrap();

        // Verify output
        assert!(out_dir.exists());
        let entries: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .collect();
        assert_eq!(entries.len(), 1);

        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("Hello everyone!"));
        assert!(content.contains("Reply in thread"));
        assert!(content.contains("Alice"));
        assert!(content.contains("slack:C1:1705312800.000000"));
    }
}
