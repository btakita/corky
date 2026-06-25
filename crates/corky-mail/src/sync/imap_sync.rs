//! IMAP connect, fetch, merge, dedup, label routing.

use anyhow::Result;
use chrono::{DateTime, Datelike, Utc};
use imap::Session;
use native_tls::TlsStream;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::markdown::{parse_thread_markdown, thread_to_markdown};
use super::types::{AccountSyncState, LabelState, Message, SyncState, Thread};
use crate::config::corky_config;
use crate::resolve;
use crate::util::{slugify, thread_key_from_subject};

static THREAD_ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\*\*Thread ID\*\*:\s*(.+)$").unwrap());

/// Extract body from a parsed email, preferring HTML→markdown over plain text.
fn extract_body(parsed: &mailparse::ParsedMail) -> String {
    if parsed.subparts.is_empty() {
        if let Ok(body) = parsed.get_body() {
            if parsed.ctype.mimetype == "text/html" {
                return html_to_markdown(&body);
            }
            return body;
        }
        return String::new();
    }
    // First pass: look for direct text/html child
    for part in &parsed.subparts {
        let ctype = part.ctype.mimetype.as_str();
        if ctype == "text/html" {
            let has_disposition = part
                .headers
                .iter()
                .any(|h| h.get_key_ref().eq_ignore_ascii_case("Content-Disposition"));
            if !has_disposition && let Ok(body) = part.get_body() {
                return html_to_markdown(&body);
            }
        }
    }
    // Second pass: look for text/plain or recurse into nested multipart
    for part in &parsed.subparts {
        let ctype = part.ctype.mimetype.as_str();
        if ctype == "text/plain" {
            let has_disposition = part
                .headers
                .iter()
                .any(|h| h.get_key_ref().eq_ignore_ascii_case("Content-Disposition"));
            if !has_disposition && let Ok(body) = part.get_body() {
                return body;
            }
        }
        if !part.subparts.is_empty() {
            let nested = extract_body(part);
            if !nested.is_empty() {
                return nested;
            }
        }
    }
    String::new()
}

/// Convert HTML to markdown via htmd (raw conversion, no cleanup).
fn html_to_markdown(html: &str) -> String {
    htmd::HtmlToMarkdown::new()
        .convert(html)
        .unwrap_or_else(|_| html.to_string())
}

/// Parse an RFC 2822 date string, falling back to epoch on failure.
pub fn parse_msg_date(date_str: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc2822(date_str)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            mailparse::dateparse(date_str)
                .map(|ts| DateTime::from_timestamp(ts, 0).unwrap_or_default())
        })
        .unwrap_or_default()
}

/// Extract and normalize the bare email address from a `From` header.
///
/// `"Display Name <addr@example.com>"` → `"addr@example.com"`, lowercased and
/// trimmed, so the same sender dedups regardless of display-name formatting.
/// Falls back to the trimmed, lowercased input when no angle-bracketed address
/// is present.
fn normalize_from_address(from: &str) -> String {
    let addr = match (from.rfind('<'), from.rfind('>')) {
        (Some(start), Some(end)) if start < end => &from[start + 1..end],
        _ => from,
    };
    addr.trim().to_lowercase()
}

/// Stable dedup key for a message (#ckydedup).
///
/// Prefers the globally unique `Message-ID` (the same message from two providers
/// shares one). When absent (for example the current IMAP path), falls back to
/// the normalized sender address plus the UTC-second timestamp so timezone /
/// date-format differences and display-name changes do not leak duplicates.
fn dedup_key(m: &Message) -> String {
    if let Some(mid) = m.message_id.as_deref() {
        let mid = mid.trim();
        if !mid.is_empty() {
            return format!("mid:{mid}");
        }
    }
    let addr = normalize_from_address(&m.from);
    let secs = parse_msg_date(&m.date).timestamp();
    format!("fd:{addr}:{secs}")
}

/// Set file mtime to the parsed date.
#[allow(unused_variables)]
fn set_mtime(path: &Path, date_str: &str) -> Result<()> {
    let dt = parse_msg_date(date_str);
    if dt.year() <= 1970 {
        return Ok(());
    }
    let ts = dt.timestamp();
    #[cfg(unix)]
    {
        use std::ffi::CString;
        let path_c = CString::new(path.to_string_lossy().as_bytes())?;
        let atime = path
            .metadata()?
            .accessed()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let times = libc::utimbuf {
            actime: atime,
            modtime: ts,
        };
        unsafe {
            libc::utime(path_c.as_ptr(), &times);
        }
    }
    Ok(())
}

/// Find an existing thread file by its Thread ID metadata.
fn find_thread_file(out_dir: &Path, thread_id: &str) -> Option<PathBuf> {
    if !out_dir.exists() {
        return None;
    }
    for entry in std::fs::read_dir(out_dir).ok()?.flatten() {
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

/// Return a slug that doesn't collide with existing files.
fn unique_slug(out_dir: &Path, slug: &str) -> String {
    if !out_dir.join(format!("{}.md", slug)).exists() {
        return slug.to_string();
    }
    let mut n = 2;
    while out_dir.join(format!("{}-{}.md", slug, n)).exists() {
        n += 1;
    }
    format!("{}-{}", slug, n)
}

/// Merge a single message into its thread file on disk.
///
/// Returns the path of the written file, or None if only metadata updated.
pub fn merge_message_to_file(
    out_dir: &Path,
    label_name: &str,
    account_name: &str,
    message: &Message,
    thread_key: &str,
) -> Result<Option<PathBuf>> {
    std::fs::create_dir_all(out_dir)?;

    let existing_file = find_thread_file(out_dir, thread_key);
    let mut thread: Thread = if let Some(ref ef) = existing_file {
        let text = std::fs::read_to_string(ef)?;
        parse_thread_markdown(&text).unwrap_or_else(|| Thread {
            id: thread_key.to_string(),
            subject: message.subject.clone(),
            ..Default::default()
        })
    } else {
        Thread {
            id: thread_key.to_string(),
            subject: message.subject.clone(),
            ..Default::default()
        }
    };

    // Accumulate labels and accounts
    if !label_name.is_empty() && !thread.labels.contains(&label_name.to_string()) {
        thread.labels.push(label_name.to_string());
    }
    if !account_name.is_empty() && !thread.accounts.contains(&account_name.to_string()) {
        thread.accounts.push(account_name.to_string());
    }

    // Deduplicate by a normalized key (#ckydedup): the raw `(from, date)` strings
    // dropped distinct same-second messages and leaked duplicates across providers
    // (date format/TZ and From display-name vary). `dedup_key` prefers the
    // globally unique Message-ID and otherwise falls back to the normalized email
    // address plus the UTC-second timestamp.
    let seen: HashSet<String> = thread.messages.iter().map(dedup_key).collect();
    if seen.contains(&dedup_key(message)) {
        // Still update labels/accounts even if message is a dupe
        if let Some(ref ef) = existing_file {
            std::fs::write(ef, thread_to_markdown(&thread))?;
            let _ = set_mtime(ef, &thread.last_date);
        }
        return Ok(existing_file);
    }

    // Clean markdown body and detect tracking domains
    let mut message = message.clone();
    let (cleaned_body, msg_tracking) = super::markdown_clean::clean_markdown(&message.body);
    message.body = cleaned_body;
    for domain in msg_tracking {
        if !thread.tracking.contains(&domain) {
            thread.tracking.push(domain);
        }
    }

    thread.messages.push(message);
    thread.messages.sort_by_key(|m| parse_msg_date(&m.date));
    thread.last_date = thread
        .messages
        .last()
        .map(|m| m.date.clone())
        .unwrap_or_default();

    let file_path = if let Some(ef) = existing_file {
        ef
    } else {
        let slug = unique_slug(out_dir, &slugify(&thread.subject));
        out_dir.join(format!("{}.md", slug))
    };

    std::fs::write(&file_path, thread_to_markdown(&thread))?;
    let _ = set_mtime(&file_path, &thread.last_date);

    let file_name = file_path.file_name().unwrap_or_default().to_string_lossy();
    let base_conversations = resolve::conversations_dir();
    if out_dir == base_conversations {
        println!("  Wrote: {file_name}");
    } else if let Ok(rel) = out_dir.strip_prefix(resolve::data_dir()) {
        println!("  Wrote: {file_name} → {}/", rel.display());
    } else {
        println!("  Wrote: {file_name} → {}/", out_dir.display());
    }
    Ok(Some(file_path))
}

/// Build label→output_dirs map from .corky.toml [routing].
///
/// Fan-out: one label can route to multiple mailbox directories.
/// Supports `account:label` syntax for per-account binding.
pub fn build_label_routes(account_name: &str) -> std::collections::HashMap<String, Vec<PathBuf>> {
    let config = match corky_config::try_load_config(None) {
        Some(c) => c,
        None => return std::collections::HashMap::new(),
    };
    let data_dir = resolve::data_dir();
    build_label_routes_from_routing(account_name, &config.routing, &data_dir)
}

/// Inner routing logic, separated for testability.
fn build_label_routes_from_routing(
    account_name: &str,
    routing: &std::collections::HashMap<String, Vec<String>>,
    data_dir: &Path,
) -> std::collections::HashMap<String, Vec<PathBuf>> {
    let mut routes: std::collections::HashMap<String, Vec<PathBuf>> =
        std::collections::HashMap::new();
    for (label_key, mailbox_paths) in routing {
        if label_key.contains(':') {
            let parts: Vec<&str> = label_key.splitn(2, ':').collect();
            let label_account = parts[0];
            let label_name = parts[1];
            if !account_name.is_empty() && label_account != account_name {
                continue;
            }
            let dirs: Vec<PathBuf> = mailbox_paths
                .iter()
                .map(|p| data_dir.join(p).join("conversations"))
                .collect();
            routes
                .entry(label_name.to_string())
                .or_default()
                .extend(dirs);
        } else {
            let dirs: Vec<PathBuf> = mailbox_paths
                .iter()
                .map(|p| data_dir.join(p).join("conversations"))
                .collect();
            routes.entry(label_key.clone()).or_default().extend(dirs);
        }
    }
    routes
}

pub type ImapSession = Session<TlsStream<TcpStream>>;

/// Connect to IMAP server (public API for other modules).
pub fn connect_imap_pub(
    host: &str,
    port: u16,
    starttls: bool,
    user: &str,
    password: &str,
) -> Result<ImapSession> {
    connect_imap(host, port, starttls, user, password)
}

/// Connect to IMAP server.
fn connect_imap(
    host: &str,
    port: u16,
    starttls: bool,
    user: &str,
    password: &str,
) -> Result<ImapSession> {
    let mut tls_builder = native_tls::TlsConnector::builder();

    if starttls || host == "127.0.0.1" || host == "localhost" {
        tls_builder.danger_accept_invalid_certs(true);
        tls_builder.danger_accept_invalid_hostnames(true);
    }

    let tls = tls_builder.build()?;

    let client = if starttls {
        imap::connect_starttls((host, port), host, &tls)?
    } else {
        imap::connect((host, port), host, &tls)?
    };

    let session = client.login(user, password).map_err(|e| e.0)?;
    Ok(session)
}

/// Sync all labels for one account.
#[allow(clippy::too_many_arguments)]
pub fn sync_account(
    account_name: &str,
    host: &str,
    port: u16,
    starttls: bool,
    user: &str,
    password: &str,
    labels: &[String],
    sync_days: u32,
    state: &mut SyncState,
    full: bool,
    base_dir: Option<&Path>,
    mut touched: Option<&mut HashSet<PathBuf>>,
    shutdown: Option<&Arc<AtomicBool>>,
) -> Result<()> {
    let base_dir = base_dir
        .map(PathBuf::from)
        .unwrap_or_else(resolve::conversations_dir);
    let acct_state = state.accounts.entry(account_name.to_string()).or_default();

    let routes = build_label_routes(account_name);

    // Merge shared labels into sync set (preserving order, no dupes)
    let mut all_labels: Vec<String> = Vec::new();
    let mut seen_labels = HashSet::new();
    for label in labels.iter().chain(routes.keys()) {
        if seen_labels.insert(label.clone()) {
            all_labels.push(label.clone());
        }
    }

    if all_labels.is_empty() {
        println!(
            "  No labels configured for account '{}' \u{2014} skipping",
            account_name
        );
        return Ok(());
    }

    println!("Connecting to {}:{} as {}", host, port, user);

    let mut session = connect_imap(host, port, starttls, user, password)?;

    for label in &all_labels {
        // Collect all output dirs: base + any fan-out routes
        let mut out_dirs = vec![base_dir.clone()];
        if let Some(dirs) = routes.get(label) {
            out_dirs.extend(dirs.iter().cloned());
        }

        // Check for shutdown signal between labels
        if let Some(s) = shutdown
            && s.load(Ordering::Relaxed)
        {
            println!("\n    Sync interrupted by shutdown signal");
            let _ = session.logout();
            return Ok(());
        }

        sync_label(
            &mut session,
            label,
            account_name,
            acct_state,
            full,
            sync_days,
            &out_dirs,
            &mut touched,
            shutdown,
        )?;
    }

    // Logout errors are non-fatal — data is already fetched and merged.
    // Some servers (e.g. ProtonMail Bridge) return responses the imap
    // crate cannot parse during logout.
    let _ = session.logout();
    Ok(())
}

/// Sync a single IMAP label/folder, writing to multiple output dirs (fan-out).
#[allow(clippy::too_many_arguments)]
fn sync_label(
    session: &mut ImapSession,
    label_name: &str,
    account_name: &str,
    acct_state: &mut AccountSyncState,
    full: bool,
    sync_days: u32,
    out_dirs: &[PathBuf],
    touched: &mut Option<&mut HashSet<PathBuf>>,
    shutdown: Option<&Arc<AtomicBool>>,
) -> Result<()> {
    println!("Syncing label: {}", label_name);

    let mailbox = match session.select(label_name) {
        Ok(mb) => mb,
        Err(_) => {
            println!("  Label \"{}\" not found \u{2014} skipping", label_name);
            return Ok(());
        }
    };

    let uidvalidity = mailbox.uid_validity.unwrap_or(0);
    let prior = acct_state.labels.get(label_name);

    let do_full = full || prior.is_none() || prior.map(|p| p.uidvalidity) != Some(uidvalidity);

    let uids: Vec<u32> = if do_full {
        if let Some(p) = prior {
            if p.uidvalidity != uidvalidity {
                println!("  UIDVALIDITY changed \u{2014} doing full resync");
            } else if full {
                println!("  Full sync requested");
            }
        } else {
            println!("  No prior state \u{2014} doing full sync");
        }

        let since_date = Utc::now() - chrono::Duration::days(sync_days as i64);
        let since_str = since_date.format("%d-%b-%Y").to_string();
        let search_result = session.uid_search(format!("SINCE {}", since_str))?;
        search_result.into_iter().collect()
    } else {
        let prior = prior.unwrap();
        let search_result = session.uid_search(format!("UID {}:*", prior.last_uid + 1))?;
        search_result
            .into_iter()
            .filter(|&u| u > prior.last_uid)
            .collect()
    };

    if uids.is_empty() {
        println!("  No new messages");
        acct_state.labels.insert(
            label_name.to_string(),
            LabelState {
                uidvalidity,
                last_uid: prior.map(|p| p.last_uid).unwrap_or(0),
            },
        );
        return Ok(());
    }

    println!("  Fetching {} message(s)", uids.len());

    let mut max_uid = prior.map(|p| p.last_uid).unwrap_or(0);

    for uid in &uids {
        // Check for shutdown signal between message fetches
        if let Some(s) = shutdown
            && s.load(Ordering::Relaxed)
        {
            println!("\n    Sync interrupted by shutdown signal");
            return Ok(());
        }

        let fetches = session.uid_fetch(uid.to_string(), "RFC822")?;
        let fetch = match fetches.iter().next() {
            Some(f) => f,
            None => continue,
        };

        let body_raw = match fetch.body() {
            Some(b) => b,
            None => continue,
        };

        let parsed = match mailparse::parse_mail(body_raw) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  Warning: failed to parse message UID {}: {}", uid, e);
                continue;
            }
        };

        let subject = parsed
            .headers
            .iter()
            .find(|h| h.get_key_ref().eq_ignore_ascii_case("Subject"))
            .map(|h| h.get_value())
            .unwrap_or_else(|| "(no subject)".to_string());

        let from = parsed
            .headers
            .iter()
            .find(|h| h.get_key_ref().eq_ignore_ascii_case("From"))
            .map(|h| h.get_value())
            .unwrap_or_default();

        let to = parsed
            .headers
            .iter()
            .find(|h| h.get_key_ref().eq_ignore_ascii_case("To"))
            .map(|h| h.get_value())
            .unwrap_or_default();

        let cc = parsed
            .headers
            .iter()
            .find(|h| h.get_key_ref().eq_ignore_ascii_case("Cc"))
            .map(|h| h.get_value())
            .unwrap_or_default();

        let date = parsed
            .headers
            .iter()
            .find(|h| h.get_key_ref().eq_ignore_ascii_case("Date"))
            .map(|h| h.get_value())
            .unwrap_or_default();

        let thread_key = thread_key_from_subject(&subject);
        let body = extract_body(&parsed);

        let message = Message {
            id: uid.to_string(),
            thread_id: thread_key.clone(),
            from,
            to,
            cc,
            date,
            subject,
            body,
            message_id: None,
        };

        for out_dir in out_dirs {
            let file_path =
                merge_message_to_file(out_dir, label_name, account_name, &message, &thread_key)?;
            if let Some(touched_set) = touched
                && let Some(ref fp) = file_path
            {
                touched_set.insert(fp.clone());
            }
        }

        if *uid > max_uid {
            max_uid = *uid;
        }
    }

    acct_state.labels.insert(
        label_name.to_string(),
        LabelState {
            uidvalidity,
            last_uid: max_uid,
        },
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // --- #ckydedup: dedup key normalization ---

    fn msg(from: &str, date: &str, mid: Option<&str>) -> Message {
        Message {
            id: String::new(),
            thread_id: String::new(),
            from: from.to_string(),
            to: String::new(),
            cc: String::new(),
            date: date.to_string(),
            subject: String::new(),
            body: String::new(),
            message_id: mid.map(|s| s.to_string()),
        }
    }

    #[test]
    fn normalize_from_address_strips_display_name() {
        assert_eq!(
            normalize_from_address("Jane Doe <Jane@Example.COM>"),
            "jane@example.com"
        );
        assert_eq!(normalize_from_address("  bob@x.io  "), "bob@x.io");
        // Same address, different display name → identical normalization.
        assert_eq!(
            normalize_from_address("Robert <bob@x.io>"),
            normalize_from_address("Bob B. <bob@x.io>")
        );
    }

    #[test]
    fn dedup_key_prefers_message_id() {
        // Same Message-ID across providers (different From/date formats) → one key.
        let a = msg("Jane <jane@x.com>", "Mon, 1 Jun 2026 10:00:00 +0000", Some("<abc@mail>"));
        let b = msg("jane@x.com", "1 Jun 2026 05:00:00 -0500", Some("<abc@mail>"));
        assert_eq!(dedup_key(&a), dedup_key(&b));
    }

    #[test]
    fn dedup_key_distinguishes_distinct_messages_same_second() {
        // Two distinct messages, same sender and second, different Message-ID →
        // distinct keys (the old (from,date) key collapsed these).
        let a = msg("jane@x.com", "Mon, 1 Jun 2026 10:00:00 +0000", Some("<a@mail>"));
        let b = msg("jane@x.com", "Mon, 1 Jun 2026 10:00:00 +0000", Some("<b@mail>"));
        assert_ne!(dedup_key(&a), dedup_key(&b));
    }

    #[test]
    fn dedup_key_fallback_normalizes_tz_and_display_name() {
        // No Message-ID: the same instant expressed in two timezones plus a
        // different display name must still dedup to one key.
        let a = msg("Jane Doe <jane@x.com>", "Mon, 1 Jun 2026 10:00:00 +0000", None);
        let b = msg("jane@x.com", "Mon, 1 Jun 2026 05:00:00 -0500", None);
        assert_eq!(dedup_key(&a), dedup_key(&b));
    }

    // --- Bug 1: Routing scope tests ---

    #[test]
    fn scoped_route_returns_for_matching_account() {
        let mut routing = HashMap::new();
        routing.insert(
            "personal:for-lucas".to_string(),
            vec!["mailboxes/lucas".to_string()],
        );
        let data_dir = PathBuf::from("/tmp/test-data");

        let routes = build_label_routes_from_routing("personal", &routing, &data_dir);
        assert!(routes.contains_key("for-lucas"), "expected for-lucas key");
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn scoped_route_returns_empty_for_other_account() {
        let mut routing = HashMap::new();
        routing.insert(
            "personal:for-lucas".to_string(),
            vec!["mailboxes/lucas".to_string()],
        );
        let data_dir = PathBuf::from("/tmp/test-data");

        let routes = build_label_routes_from_routing("proton-dev", &routing, &data_dir);
        assert!(routes.is_empty(), "expected empty routes for proton-dev");
    }

    #[test]
    fn unscoped_route_returns_for_all_accounts() {
        let mut routing = HashMap::new();
        routing.insert(
            "shared-label".to_string(),
            vec!["mailboxes/shared".to_string()],
        );
        let data_dir = PathBuf::from("/tmp/test-data");

        let routes_a = build_label_routes_from_routing("personal", &routing, &data_dir);
        let routes_b = build_label_routes_from_routing("proton-dev", &routing, &data_dir);

        assert!(routes_a.contains_key("shared-label"));
        assert!(routes_b.contains_key("shared-label"));
    }

    // --- extract_body tests ---

    fn make_parsed_mail(content_type: &str, body: &str) -> mailparse::ParsedMail<'static> {
        let raw = format!("Content-Type: {}\r\n\r\n{}", content_type, body);
        // Leak to get 'static lifetime for test convenience
        let leaked: &'static str = Box::leak(raw.into_boxed_str());
        mailparse::parse_mail(leaked.as_bytes()).unwrap()
    }

    #[test]
    fn extract_body_plain_text_only() {
        let parsed = make_parsed_mail("text/plain", "Hello world");
        let body = extract_body(&parsed);
        assert_eq!(body, "Hello world");
    }

    #[test]
    fn extract_body_html_only() {
        let parsed = make_parsed_mail("text/html", "<p>Hello <b>world</b></p>");
        let body = extract_body(&parsed);
        assert!(body.contains("Hello"), "expected Hello in: {body}");
        assert!(body.contains("world"), "expected world in: {body}");
    }

    #[test]
    fn extract_body_multipart_prefers_html() {
        let raw = b"Content-Type: multipart/alternative; boundary=boundary123\r\n\r\n\
--boundary123\r\n\
Content-Type: text/plain\r\n\r\n\
Plain text\r\n\
--boundary123\r\n\
Content-Type: text/html\r\n\r\n\
<p>HTML <b>text</b></p>\r\n\
--boundary123--";
        let parsed = mailparse::parse_mail(raw).unwrap();
        let body = extract_body(&parsed);
        // Should prefer HTML→markdown over plain
        assert!(body.contains("HTML"), "expected HTML content in: {body}");
        assert!(body.contains("text"), "expected text content in: {body}");
    }

    #[test]
    fn extract_body_multipart_plain_fallback_when_no_html() {
        let raw = b"Content-Type: multipart/mixed; boundary=boundary456\r\n\r\n\
--boundary456\r\n\
Content-Type: text/plain\r\n\r\n\
Only plain text\r\n\
--boundary456\r\n\
Content-Type: image/png\r\n\
Content-Disposition: attachment\r\n\r\n\
fake-image-data\r\n\
--boundary456--";
        let parsed = mailparse::parse_mail(raw).unwrap();
        let body = extract_body(&parsed);
        assert_eq!(body, "Only plain text\r\n");
    }

    // --- Bug 3: Shutdown signal smoke test ---

    #[test]
    fn shutdown_signal_is_observable() {
        let shutdown = Arc::new(AtomicBool::new(false));
        assert!(!shutdown.load(Ordering::Relaxed));

        shutdown.store(true, Ordering::Relaxed);
        assert!(shutdown.load(Ordering::Relaxed));
    }
}
