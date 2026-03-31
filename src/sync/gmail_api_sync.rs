//! Gmail API sync — fetch messages via REST API and write to Markdown.

use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::imap_sync::{build_label_routes, merge_message_to_file};
use super::types::{GmailLabelState, Message, SyncState};
use crate::filter::gmail_auth::{self, GMAIL_SYNC_SCOPE};
use crate::resolve;

const GMAIL_API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

// --- API response types ---

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageListResponse {
    #[serde(default)]
    messages: Vec<MessageRef>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageRef {
    id: String,
    thread_id: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GmailMessage {
    id: String,
    thread_id: String,
    #[serde(default)]
    label_ids: Vec<String>,
    #[serde(default)]
    payload: Option<Payload>,
    #[serde(default)]
    history_id: Option<String>,
    #[serde(default)]
    internal_date: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    #[serde(default)]
    headers: Vec<Header>,
    #[serde(default)]
    parts: Vec<Part>,
    #[serde(default)]
    body: Option<Body>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    headers: Vec<Header>,
    #[serde(default)]
    body: Option<Body>,
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Body {
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    attachment_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryResponse {
    #[serde(default)]
    history: Vec<HistoryRecord>,
    #[serde(default)]
    history_id: Option<String>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryRecord {
    #[serde(default)]
    messages_added: Vec<MessageAdded>,
}

#[derive(Debug, serde::Deserialize)]
struct MessageAdded {
    message: MessageRef,
}

#[derive(Debug, serde::Deserialize)]
struct LabelListResponse {
    #[serde(default)]
    labels: Vec<LabelEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct LabelEntry {
    id: String,
    name: String,
}

// --- HTTP helpers ---

fn api_get(token: &str, url: &str) -> Result<ureq::Response> {
    match ureq::get(url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(401, _)) => {
            bail!("Gmail API: unauthorized (401). Token may be expired or revoked.");
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Gmail API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Verify that the OAuth token belongs to the expected email address.
fn verify_account_email(token: &str, expected_email: &str) -> Result<()> {
    let resp = api_get(token, &format!("{}/profile", GMAIL_API))?;
    let profile: serde_json::Value = resp.into_json().context("Failed to parse profile")?;
    let actual_email = profile["emailAddress"]
        .as_str()
        .unwrap_or("");
    if !actual_email.eq_ignore_ascii_case(expected_email) {
        bail!(
            "Account mismatch: OAuth token is for '{}' but account is configured as '{}'.\n\
             Delete the stale token with: corky filter auth\n\
             Then re-run sync to re-authenticate with the correct account.",
            actual_email,
            expected_email
        );
    }
    Ok(())
}

/// Fetch label name→ID and ID→name maps.
fn fetch_label_maps(token: &str) -> Result<(HashMap<String, String>, HashMap<String, String>)> {
    let resp = api_get(token, &format!("{}/labels", GMAIL_API))?;
    let body: LabelListResponse = resp.into_json().context("Failed to parse labels")?;
    let mut name_to_id = HashMap::new();
    let mut id_to_name = HashMap::new();
    for label in body.labels {
        name_to_id.insert(label.name.clone(), label.id.clone());
        id_to_name.insert(label.id, label.name);
    }
    Ok((name_to_id, id_to_name))
}

// --- Message parsing ---

/// Base64url decode (Gmail uses URL-safe base64, sometimes with padding).
fn base64url_decode(data: &str) -> Result<String> {
    use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
    use base64::Engine;
    let bytes = URL_SAFE_NO_PAD
        .decode(data)
        .or_else(|_| URL_SAFE.decode(data))
        .context("base64url decode failed")?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// Fetch attachment data by ID (for parts where Gmail returns attachmentId instead of inline data).
fn fetch_attachment(token: &str, message_id: &str, attachment_id: &str) -> Result<String> {
    let url = format!(
        "{}/messages/{}/attachments/{}",
        GMAIL_API, message_id, attachment_id
    );
    let resp = api_get(token, &url)?.into_string()?;
    let body: Body = serde_json::from_str(&resp)?;
    body.data
        .map(|d| base64url_decode(&d))
        .unwrap_or_else(|| Ok(String::new()))
}

/// Resolve body data — inline or via attachment fetch.
fn resolve_body_data(body: &Body, token: &str, message_id: &str) -> String {
    if let Some(data) = &body.data {
        return base64url_decode(data).unwrap_or_default();
    }
    if let Some(att_id) = &body.attachment_id {
        return fetch_attachment(token, message_id, att_id).unwrap_or_default();
    }
    String::new()
}

/// Extract text/plain body from Gmail MIME payload.
fn extract_body_from_payload(payload: &Payload, token: &str, message_id: &str) -> String {
    // Single-part message
    if payload.parts.is_empty() {
        if let Some(mime) = &payload.mime_type
            && mime == "text/plain"
                && let Some(body) = &payload.body {
                    return resolve_body_data(body, token, message_id);
                }
        return String::new();
    }

    // Multipart — walk parts looking for text/plain
    extract_body_from_parts(&payload.parts, token, message_id)
}

fn extract_body_from_parts(parts: &[Part], token: &str, message_id: &str) -> String {
    for part in parts {
        if let Some(mime) = &part.mime_type
            && mime == "text/plain"
                && let Some(body) = &part.body {
                    let text = resolve_body_data(body, token, message_id);
                    if !text.is_empty() {
                        return text;
                    }
                }
        // Recurse into nested parts
        if !part.parts.is_empty() {
            let nested = extract_body_from_parts(&part.parts, token, message_id);
            if !nested.is_empty() {
                return nested;
            }
        }
    }
    String::new()
}

/// Extract a header value by name (case-insensitive).
fn get_header(headers: &[Header], name: &str) -> String {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.clone())
        .unwrap_or_default()
}

/// Convert a Gmail API message to our sync Message struct.
fn gmail_to_message(msg: &GmailMessage, token: &str) -> Message {
    let payload = msg.payload.as_ref();
    let headers = payload.map(|p| &p.headers[..]).unwrap_or(&[]);

    let body = payload
        .map(|p| extract_body_from_payload(p, token, &msg.id))
        .unwrap_or_default();

    // Convert internalDate (epoch millis) to RFC 2822
    let date = if let Some(date_hdr) = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("Date"))
    {
        date_hdr.value.clone()
    } else if let Some(ref internal) = msg.internal_date {
        if let Ok(millis) = internal.parse::<i64>() {
            chrono::DateTime::from_timestamp(millis / 1000, 0)
                .map(|dt| dt.to_rfc2822())
                .unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    Message {
        id: msg.id.clone(),
        thread_id: msg.thread_id.clone(),
        from: get_header(headers, "From"),
        to: get_header(headers, "To"),
        cc: get_header(headers, "Cc"),
        date,
        subject: get_header(headers, "Subject"),
        body,
    }
}

// --- Sync logic ---

/// Sync a single Gmail API account.
#[allow(clippy::too_many_arguments)]
pub fn sync_account(
    account_name: &str,
    user: &str,
    labels: &[String],
    sync_days: u32,
    state: &mut SyncState,
    full: bool,
    mut touched: Option<&mut HashSet<PathBuf>>,
    shutdown: Option<&Arc<AtomicBool>>,
) -> Result<()> {
    println!("Connecting via Gmail API as {}", user);

    // Get OAuth2 token (will trigger browser auth on first run)
    // Pass user email as login_hint to pre-select the correct Google account
    let token = gmail_auth::get_access_token_for_user(Some(account_name), GMAIL_SYNC_SCOPE, Some(user))?;

    // Verify the authenticated account matches the configured user
    verify_account_email(&token, user)?;

    // Fetch label maps for ID↔name resolution
    let (_name_to_id, id_to_name) = fetch_label_maps(&token)?;

    // Build label routes
    let label_routes = build_label_routes(account_name);
    let conversations_dir = resolve::conversations_dir();

    let acct_state = state
        .accounts
        .entry(account_name.to_string())
        .or_default();

    for label_name in labels {
        println!("  Label: {}", label_name);

        // Resolve label name to Gmail label ID
        let label_id = resolve_label_id(label_name, &_name_to_id)?;

        // Determine output directories
        let out_dirs: Vec<PathBuf> = {
            let mut dirs = vec![conversations_dir.clone()];
            if let Some(extra) = label_routes.get(label_name) {
                dirs.extend(extra.iter().cloned());
            }
            dirs
        };

        // Get Gmail sync state for this label
        let gmail_state = acct_state
            .gmail_labels
            .entry(label_name.clone())
            .or_insert_with(|| GmailLabelState {
                last_history_id: None,
            });

        let message_ids = if !full && gmail_state.last_history_id.is_some() {
            // Incremental sync via history API
            match fetch_new_message_ids_incremental(
                &token,
                &label_id,
                gmail_state.last_history_id.unwrap(),
            ) {
                Ok((ids, new_history_id)) => {
                    if let Some(hid) = new_history_id {
                        gmail_state.last_history_id = Some(hid);
                    }
                    ids
                }
                Err(e) => {
                    eprintln!(
                        "    History API failed ({}), falling back to full sync",
                        e
                    );
                    gmail_state.last_history_id = None;
                    fetch_all_message_ids(&token, &label_id, sync_days, shutdown)?
                }
            }
        } else {
            // Full sync
            fetch_all_message_ids(&token, &label_id, sync_days, shutdown)?
        };

        println!("    {} messages to process", message_ids.len());

        let mut max_history_id: Option<u64> = gmail_state.last_history_id;
        let total = message_ids.len();

        for (idx, msg_ref) in message_ids.iter().enumerate() {
            // Check for shutdown signal
            if let Some(s) = shutdown
                && s.load(Ordering::Relaxed) {
                    println!("\n    Sync interrupted by shutdown signal");
                    return Ok(());
                }

            if total > 100 && (idx + 1) % 100 == 0 {
                print!("    Processing {}/{}...\r", idx + 1, total);
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }

            let gmail_msg = match fetch_message(&token, &msg_ref.id)? {
                Some(msg) => msg,
                None => {
                    eprintln!(
                        "    Skipping message {} (deleted before fetch)",
                        msg_ref.id
                    );
                    continue;
                }
            };

            // Track highest history_id
            if let Some(ref hid_str) = gmail_msg.history_id
                && let Ok(hid) = hid_str.parse::<u64>() {
                    max_history_id = Some(max_history_id.map_or(hid, |cur| cur.max(hid)));
                }

            let message = gmail_to_message(&gmail_msg, &token);

            // Use Gmail's threadId as the thread key (better than subject heuristic)
            let thread_key = &gmail_msg.thread_id;

            // Resolve label names from labelIds
            let msg_labels: Vec<String> = gmail_msg
                .label_ids
                .iter()
                .filter_map(|id| id_to_name.get(id).cloned())
                .collect();

            for out_dir in &out_dirs {
                let result = merge_message_to_file(
                    out_dir,
                    label_name,
                    account_name,
                    &message,
                    thread_key,
                )?;
                if let Some(ref path) = result
                    && let Some(ref mut t) = touched.as_deref_mut() {
                        t.insert(path.clone());
                    }
            }

            // Also write to any extra routes for message-specific labels
            for msg_label in &msg_labels {
                if msg_label == label_name {
                    continue; // Already handled above
                }
                if let Some(extra_dirs) = label_routes.get(msg_label) {
                    for extra_dir in extra_dirs {
                        if out_dirs.contains(extra_dir) {
                            continue; // Already written
                        }
                        let _ = merge_message_to_file(
                            extra_dir,
                            msg_label,
                            account_name,
                            &message,
                            thread_key,
                        )?;
                    }
                }
            }
        }

        if total > 100 {
            println!("    Processed {}/{} messages", total, total);
        }

        // Update history_id state
        if let Some(hid) = max_history_id {
            gmail_state.last_history_id = Some(hid);
        }
    }

    Ok(())
}

/// Resolve a label name to its Gmail label ID.
fn resolve_label_id(name: &str, name_to_id: &HashMap<String, String>) -> Result<String> {
    // System labels have well-known IDs that match their name
    let upper = name.to_uppercase();
    if matches!(
        upper.as_str(),
        "INBOX" | "STARRED" | "IMPORTANT" | "SENT" | "DRAFT" | "SPAM" | "TRASH" | "UNREAD"
    ) || upper.starts_with("CATEGORY_")
    {
        return Ok(upper);
    }

    // Look up user label by name (case-insensitive)
    if let Some(id) = name_to_id.get(name) {
        return Ok(id.clone());
    }
    let lower = name.to_lowercase();
    for (label_name, label_id) in name_to_id {
        if label_name.to_lowercase() == lower {
            return Ok(label_id.clone());
        }
    }

    bail!("Gmail label '{}' not found", name)
}

/// Fetch all message IDs for a label within sync_days.
fn fetch_all_message_ids(
    token: &str,
    label_id: &str,
    sync_days: u32,
    shutdown: Option<&Arc<AtomicBool>>,
) -> Result<Vec<MessageRef>> {
    let since = chrono::Utc::now() - chrono::Duration::days(sync_days as i64);
    let query = format!("after:{}", since.format("%Y/%m/%d"));

    let mut all_messages = Vec::new();
    let mut page_token: Option<String> = None;
    let mut page_count = 0u32;

    loop {
        let mut url = format!(
            "{}/messages?labelIds={}&q={}&maxResults=500",
            GMAIL_API, label_id, query
        );
        if let Some(ref pt) = page_token {
            url.push_str(&format!("&pageToken={}", pt));
        }

        let resp = api_get(token, &url)?;
        let body: MessageListResponse = resp.into_json().context("Failed to parse message list")?;

        all_messages.extend(body.messages);
        page_count += 1;

        // Check for shutdown signal
        if let Some(s) = shutdown
            && s.load(Ordering::Relaxed) {
                println!("\n    Listing interrupted by shutdown signal");
                return Ok(all_messages); // Return what we have so far
            }

        // Progress indicator every 5 pages
        if page_count.is_multiple_of(5) {
            print!("    Listing messages... {} found\r", all_messages.len());
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }

        match body.next_page_token {
            Some(pt) => page_token = Some(pt),
            None => break,
        }
    }

    if page_count > 5 {
        println!(); // Clear the progress line
    }

    Ok(all_messages)
}

/// Fetch new message IDs since a history_id (incremental sync).
///
/// Returns (message_refs, new_history_id).
fn fetch_new_message_ids_incremental(
    token: &str,
    label_id: &str,
    start_history_id: u64,
) -> Result<(Vec<MessageRef>, Option<u64>)> {
    let mut all_messages = Vec::new();
    let mut page_token: Option<String> = None;
    let mut latest_history_id: Option<u64> = None;
    let mut seen_ids = HashSet::new();

    loop {
        let mut url = format!(
            "{}/history?startHistoryId={}&historyTypes=messageAdded&labelId={}",
            GMAIL_API, start_history_id, label_id
        );
        if let Some(ref pt) = page_token {
            url.push_str(&format!("&pageToken={}", pt));
        }

        let resp = api_get(token, &url)?;
        let body: HistoryResponse = resp.into_json().context("Failed to parse history")?;

        if let Some(ref hid) = body.history_id
            && let Ok(h) = hid.parse::<u64>() {
                latest_history_id = Some(latest_history_id.map_or(h, |cur: u64| cur.max(h)));
            }

        for record in &body.history {
            for added in &record.messages_added {
                if seen_ids.insert(added.message.id.clone()) {
                    all_messages.push(MessageRef {
                        id: added.message.id.clone(),
                        thread_id: added.message.thread_id.clone(),
                    });
                }
            }
        }

        match body.next_page_token {
            Some(pt) => page_token = Some(pt),
            None => break,
        }
    }

    Ok((all_messages, latest_history_id))
}

/// Fetch a single message by ID.
fn fetch_message(token: &str, message_id: &str) -> Result<Option<GmailMessage>> {
    let url = format!("{}/messages/{}?format=full", GMAIL_API, message_id);
    match ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(resp) => {
            let msg: GmailMessage = resp.into_json().context("Failed to parse message")?;
            Ok(Some(msg))
        }
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(ureq::Error::Status(401, _)) => {
            bail!("Gmail API: unauthorized (401). Token may be expired or revoked.");
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Gmail API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}

// --- Thread API ---

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadResponse {
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    messages: Vec<GmailMessage>,
}

/// Fetch all messages in a thread by thread ID.
/// Returns converted Message structs ready for merge.
pub fn fetch_thread_messages(token: &str, thread_id: &str) -> Result<Vec<super::types::Message>> {
    let url = format!("{}/threads/{}?format=full", GMAIL_API, thread_id);
    let resp = api_get(token, &url)?;
    let thread: ThreadResponse = resp.into_json().context("Failed to parse thread")?;

    let messages: Vec<super::types::Message> = thread
        .messages
        .iter()
        .map(|msg| gmail_to_message(msg, token))
        .collect();

    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    fn b64(s: &str) -> String {
        URL_SAFE_NO_PAD.encode(s)
    }

    #[test]
    fn test_extract_body_simple_text_plain() {
        let payload = Payload {
            headers: vec![],
            parts: vec![],
            body: Some(Body {
                data: Some(b64("Hello world")),
                attachment_id: None,
            }),
            mime_type: Some("text/plain".into()),
        };
        let body = extract_body_from_payload(&payload, "", "");
        assert_eq!(body, "Hello world");
    }

    #[test]
    fn test_extract_body_multipart_alternative() {
        let payload = Payload {
            headers: vec![],
            parts: vec![
                Part {
                    mime_type: Some("text/plain".into()),
                    headers: vec![],
                    body: Some(Body {
                        data: Some(b64("Plain text")),
                        attachment_id: None,
                    }),
                    parts: vec![],
                },
                Part {
                    mime_type: Some("text/html".into()),
                    headers: vec![],
                    body: Some(Body {
                        data: Some(b64("<p>HTML</p>")),
                        attachment_id: None,
                    }),
                    parts: vec![],
                },
            ],
            body: None,
            mime_type: Some("multipart/alternative".into()),
        };
        let body = extract_body_from_payload(&payload, "", "");
        assert_eq!(body, "Plain text");
    }

    #[test]
    fn test_extract_body_multipart_related_nested() {
        // multipart/related → multipart/alternative → text/plain
        let payload = Payload {
            headers: vec![],
            parts: vec![
                Part {
                    mime_type: Some("multipart/alternative".into()),
                    headers: vec![],
                    body: None,
                    parts: vec![
                        Part {
                            mime_type: Some("text/plain".into()),
                            headers: vec![],
                            body: Some(Body {
                                data: Some(b64("Nested plain text")),
                                attachment_id: None,
                            }),
                            parts: vec![],
                        },
                        Part {
                            mime_type: Some("text/html".into()),
                            headers: vec![],
                            body: Some(Body {
                                data: Some(b64("<p>Nested HTML</p>")),
                                attachment_id: None,
                            }),
                            parts: vec![],
                        },
                    ],
                },
                Part {
                    mime_type: Some("image/jpeg".into()),
                    headers: vec![],
                    body: Some(Body {
                        data: None,
                        attachment_id: Some("att123".into()),
                    }),
                    parts: vec![],
                },
            ],
            body: None,
            mime_type: Some("multipart/related".into()),
        };
        let body = extract_body_from_payload(&payload, "", "");
        assert_eq!(body, "Nested plain text");
    }

    #[test]
    fn test_extract_body_empty_data_no_attachment() {
        let payload = Payload {
            headers: vec![],
            parts: vec![],
            body: Some(Body {
                data: None,
                attachment_id: None,
            }),
            mime_type: Some("text/plain".into()),
        };
        let body = extract_body_from_payload(&payload, "", "");
        assert_eq!(body, "");
    }

    #[test]
    fn test_resolve_body_data_prefers_inline() {
        let body = Body {
            data: Some(b64("inline data")),
            attachment_id: Some("should_not_be_used".into()),
        };
        let result = resolve_body_data(&body, "", "");
        assert_eq!(result, "inline data");
    }

    #[test]
    fn test_body_deserializes_attachment_id() {
        let json = r#"{"data": null, "attachmentId": "abc123", "size": 1024}"#;
        let body: Body = serde_json::from_str(json).unwrap();
        assert!(body.data.is_none());
        assert_eq!(body.attachment_id, Some("abc123".into()));
    }

    #[test]
    fn test_body_deserializes_inline_data() {
        let encoded = b64("hello");
        let json = format!(r#"{{"data": "{}"}}"#, encoded);
        let body: Body = serde_json::from_str(&json).unwrap();
        assert_eq!(body.data, Some(encoded));
        assert!(body.attachment_id.is_none());
    }

    #[test]
    fn test_thread_response_deserialize() {
        let json = format!(
            r#"{{
                "id": "thread123",
                "messages": [
                    {{
                        "id": "msg1",
                        "threadId": "thread123",
                        "labelIds": ["INBOX"],
                        "payload": {{
                            "headers": [
                                {{"name": "From", "value": "alice@example.com"}},
                                {{"name": "Subject", "value": "Test thread"}}
                            ],
                            "body": {{"data": "{}"}},
                            "mimeType": "text/plain"
                        }},
                        "historyId": "12345",
                        "internalDate": "1700000000000"
                    }},
                    {{
                        "id": "msg2",
                        "threadId": "thread123",
                        "labelIds": ["INBOX"],
                        "payload": {{
                            "headers": [
                                {{"name": "From", "value": "bob@example.com"}},
                                {{"name": "Subject", "value": "Re: Test thread"}}
                            ],
                            "body": {{"data": "{}"}},
                            "mimeType": "text/plain"
                        }},
                        "historyId": "12346",
                        "internalDate": "1700001000000"
                    }}
                ]
            }}"#,
            b64("Hello from Alice"),
            b64("Reply from Bob"),
        );
        let thread: ThreadResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(thread.id, "thread123");
        assert_eq!(thread.messages.len(), 2);
        assert_eq!(thread.messages[0].id, "msg1");
        assert_eq!(thread.messages[1].id, "msg2");
    }

    #[test]
    fn test_thread_response_to_messages() {
        let json = format!(
            r#"{{
                "id": "thread456",
                "messages": [
                    {{
                        "id": "m1",
                        "threadId": "thread456",
                        "payload": {{
                            "headers": [
                                {{"name": "From", "value": "sender@example.com"}},
                                {{"name": "To", "value": "recipient@example.com"}},
                                {{"name": "Subject", "value": "Refetch test"}},
                                {{"name": "Date", "value": "Mon, 1 Jan 2024 00:00:00 +0000"}}
                            ],
                            "body": {{"data": "{}"}},
                            "mimeType": "text/plain"
                        }}
                    }}
                ]
            }}"#,
            b64("Body content here"),
        );
        let thread: ThreadResponse = serde_json::from_str(&json).unwrap();
        let messages: Vec<_> = thread
            .messages
            .iter()
            .map(|msg| gmail_to_message(msg, ""))
            .collect();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from, "sender@example.com");
        assert_eq!(messages[0].to, "recipient@example.com");
        assert_eq!(messages[0].subject, "Refetch test");
        assert_eq!(messages[0].body, "Body content here");
        assert_eq!(messages[0].thread_id, "thread456");
    }

    #[test]
    fn test_thread_response_empty_messages() {
        let json = r#"{"id": "empty_thread", "messages": []}"#;
        let thread: ThreadResponse = serde_json::from_str(json).unwrap();
        assert_eq!(thread.id, "empty_thread");
        assert!(thread.messages.is_empty());
    }
}
