//! Push a draft markdown file as an email draft, or send it directly.

pub mod attach;
pub mod migrate;
pub mod new;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::accounts::{
    get_account_for_email, get_default_account, load_accounts, resolve_password,
};

static META_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\*\*(.+?)\*\*:\s*(.+)$").unwrap());

const VALID_SEND_STATUSES: &[&str] = &["review", "approved", "scheduled"];

fn default_draft_status() -> String {
    "draft".to_string()
}

/// YAML frontmatter metadata for an email draft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailDraftMeta {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    #[serde(default = "default_draft_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

/// Returns true if the content starts with YAML frontmatter.
pub fn is_yaml_format(content: &str) -> bool {
    content.starts_with("---\n") || content.starts_with("---\r\n")
}

/// Parse YAML frontmatter from email draft content. Returns (meta_struct, meta_hashmap, subject, body).
fn parse_yaml_draft(text: &str) -> Result<(EmailDraftMeta, HashMap<String, String>, String, String)> {
    let after_first = &text[4..]; // skip "---\n"
    let end = after_first.find("\n---").ok_or_else(|| {
        anyhow::anyhow!("Missing closing YAML frontmatter delimiter `---`")
    })?;

    let yaml_str = &after_first[..end];
    let body_start = end + 4; // skip "\n---"
    let body_section = if body_start < after_first.len() {
        after_first[body_start..].trim_start_matches('\n').to_string()
    } else {
        String::new()
    };

    let meta: EmailDraftMeta = serde_yaml::from_str(yaml_str)?;

    // Prefer subject from YAML frontmatter, fall back to first # heading in body
    let subject = meta.subject.clone()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            body_section
                .lines()
                .find_map(|line| line.strip_prefix("# ").map(|s| s.trim().to_string()))
        })
        .unwrap_or_default();

    // Body is everything after the subject heading line (if present)
    let body = if let Some(pos) = body_section.find('\n') {
        let first_line = body_section[..pos].trim();
        if first_line.starts_with("# ") {
            body_section[pos + 1..].trim_start_matches('\n').to_string()
        } else {
            body_section.clone()
        }
    } else {
        // Only one line — if it's the subject, body is empty
        if body_section.trim().starts_with("# ") {
            String::new()
        } else {
            body_section.clone()
        }
    };

    // Build HashMap for backward compatibility with compose_email / resolve_account
    let mut map = HashMap::new();
    map.insert("To".to_string(), meta.to.clone());
    if let Some(ref cc) = meta.cc {
        map.insert("CC".to_string(), cc.clone());
    }
    map.insert("Status".to_string(), meta.status.clone());
    if let Some(ref author) = meta.author {
        map.insert("Author".to_string(), author.clone());
    }
    if let Some(ref account) = meta.account {
        map.insert("Account".to_string(), account.clone());
    }
    if let Some(ref from) = meta.from {
        map.insert("From".to_string(), from.clone());
    }
    if let Some(ref in_reply_to) = meta.in_reply_to {
        map.insert("In-Reply-To".to_string(), in_reply_to.clone());
    }
    if let Some(ref scheduled_at) = meta.scheduled_at {
        map.insert("Scheduled-At".to_string(), scheduled_at.to_rfc3339());
    }

    Ok((meta, map, subject, body))
}

/// Parse a draft markdown file. Returns (meta, subject, body).
///
/// Supports two formats:
/// - YAML frontmatter (new): file starts with `---\n`
/// - Legacy `**Key**: value` regex format
pub fn parse_draft(path: &Path) -> Result<(HashMap<String, String>, String, String)> {
    let text = std::fs::read_to_string(path)?;

    if is_yaml_format(&text) {
        let (_meta_struct, map, subject, body) = parse_yaml_draft(&text)?;
        return Ok((map, subject, body));
    }

    // Legacy format
    let lines: Vec<&str> = text.split('\n').collect();

    let subject = lines
        .iter()
        .find_map(|line| {
            line.strip_prefix("# ")
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default();

    let mut meta = HashMap::new();
    for cap in META_RE.captures_iter(&text) {
        meta.insert(cap[1].to_string(), cap[2].trim().to_string());
    }

    if !meta.contains_key("To") {
        bail!("Draft is missing **To**: field: {}", path.display());
    }

    // Body is everything after the first ---
    let body_start = lines.iter().position(|line| line.trim() == "---");
    let Some(body_start) = body_start else {
        bail!("Draft is missing --- separator: {}", path.display());
    };

    let body = lines[body_start + 1..].join("\n").trim().to_string();
    Ok((meta, subject, body))
}

/// Parse a draft markdown file content as YAML, returning the typed struct.
/// Returns None if the content is not in YAML format.
pub fn parse_draft_yaml(content: &str) -> Option<EmailDraftMeta> {
    if !is_yaml_format(content) {
        return None;
    }
    let after_first = &content[4..];
    let end = after_first.find("\n---")?;
    let yaml_str = &after_first[..end];
    serde_yaml::from_str(yaml_str).ok()
}

/// Convert markdown body text to HTML.
fn markdown_to_html(body: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_SMART_PUNCTUATION;
    let parser = Parser::new_ext(body, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// An inline image with a generated Content-ID and resolved path.
struct InlineImage {
    cid: String,
    path: PathBuf,
}

/// Compose an email from draft metadata.
fn compose_email(
    meta: &HashMap<String, String>,
    subject: &str,
    body: &str,
    from_addr: &str,
    attachment_paths: &[String],
    image_paths: &[String],
) -> Result<Message> {
    let from: Mailbox = from_addr.parse().map_err(|_| anyhow::anyhow!("Invalid from address: {}", from_addr))?;
    let to: Mailbox = meta["To"]
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid To address: {}", meta["To"]))?;

    let mut builder = Message::builder()
        .from(from)
        .to(to)
        .subject(subject);

    if let Some(cc) = meta.get("CC")
        && !cc.is_empty() {
            let cc_box: Mailbox = cc.parse().map_err(|_| anyhow::anyhow!("Invalid CC address: {}", cc))?;
            builder = builder.cc(cc_box);
        }

    if let Some(in_reply_to) = meta.get("In-Reply-To")
        && !in_reply_to.is_empty() {
            builder = builder.in_reply_to(in_reply_to.to_string());
            builder = builder.references(in_reply_to.to_string());
        }

    // Prepare inline images: generate CIDs and validate paths
    let inline_images: Vec<InlineImage> = image_paths
        .iter()
        .enumerate()
        .map(|(i, p)| InlineImage {
            cid: format!("image{}@corky", i + 1),
            path: PathBuf::from(p),
        })
        .collect();

    // Build HTML body, injecting CID references for inline images
    let mut html_body = markdown_to_html(body);
    if !inline_images.is_empty() {
        // Append inline image references to the HTML body
        for img in &inline_images {
            let filename = img.path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "image".to_string());
            html_body.push_str(&format!(
                "<p><img src=\"cid:{}\" alt=\"{}\" /></p>\n",
                img.cid, filename
            ));
        }
    }

    let alternative = MultiPart::alternative()
        .singlepart(SinglePart::plain(body.to_string()))
        .singlepart(SinglePart::html(html_body));

    // Build the MIME structure:
    // - No images, no attachments: just alternative
    // - Images only: multipart/related (alternative + inline images)
    // - Attachments only: multipart/mixed (alternative + attachments)
    // - Both: multipart/mixed (multipart/related (alternative + inline images) + attachments)
    let has_images = !inline_images.is_empty();
    let has_attachments = !attachment_paths.is_empty();

    let body_part = if has_images {
        let mut related = MultiPart::related().multipart(alternative);
        for img in &inline_images {
            if !img.path.exists() {
                bail!("Inline image not found: {}", img.path.display());
            }
            let file_bytes = std::fs::read(&img.path)?;
            let content_type = mime_guess::from_path(&img.path)
                .first()
                .map(|mime| {
                    ContentType::parse(mime.as_ref())
                        .unwrap_or(ContentType::parse("application/octet-stream").unwrap())
                })
                .unwrap_or_else(|| ContentType::parse("image/png").unwrap());

            let filename = img.path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "image".to_string());

            let inline_part = Attachment::new_inline_with_name(img.cid.clone(), filename)
                .body(file_bytes, content_type);

            related = related.singlepart(inline_part);
        }
        related
    } else {
        alternative
    };

    if !has_attachments {
        let email = builder.multipart(body_part)?;
        Ok(email)
    } else {
        let mut multipart = MultiPart::mixed().multipart(body_part);

        for path_str in attachment_paths {
            let path = Path::new(path_str);
            if !path.exists() {
                bail!("Attachment not found: {}", path_str);
            }
            let file_bytes = std::fs::read(path)?;
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "attachment".to_string());

            let content_type = mime_guess::from_path(path)
                .first()
                .map(|mime| {
                    ContentType::parse(mime.as_ref())
                        .unwrap_or(ContentType::parse("application/octet-stream").unwrap())
                })
                .unwrap_or_else(|| ContentType::parse("application/octet-stream").unwrap());

            let attachment = Attachment::new(filename).body(file_bytes, content_type);
            multipart = multipart.singlepart(attachment);
        }

        let email = builder.multipart(multipart)?;
        Ok(email)
    }
}

/// Push draft to IMAP drafts folder.
fn push_to_drafts(
    email: &Message,
    imap_host: &str,
    imap_port: u16,
    starttls: bool,
    user: &str,
    password: &str,
    drafts_folder: &str,
) -> Result<()> {
    let mut tls_builder = native_tls::TlsConnector::builder();
    if starttls || imap_host == "127.0.0.1" || imap_host == "localhost" {
        tls_builder.danger_accept_invalid_certs(true);
        tls_builder.danger_accept_invalid_hostnames(true);
    }
    let tls = tls_builder.build()?;

    let client = if starttls {
        imap::connect_starttls((imap_host, imap_port), imap_host, &tls)?
    } else {
        imap::connect((imap_host, imap_port), imap_host, &tls)?
    };

    let mut session = client.login(user, password).map_err(|e| e.0)?;

    let email_bytes = email.formatted();
    session.append(drafts_folder, &email_bytes)?;
    session.logout()?;
    Ok(())
}

/// Send email via Gmail API (messages.send endpoint).
///
/// Uses OAuth2 token from the token store (scope: gmail.send).
/// The email is serialized as RFC 2822, base64url-encoded, and POSTed.
fn send_via_gmail_api(
    email: &Message,
    account_name: &str,
    user: &str,
    thread_id: Option<&str>,
) -> Result<()> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let token = crate::filter::gmail_auth::get_send_access_token(Some(account_name), Some(user))?;

    let raw_bytes = email.formatted();
    let encoded = URL_SAFE_NO_PAD.encode(&raw_bytes);

    let mut body = serde_json::json!({ "raw": encoded });
    if let Some(tid) = thread_id {
        body["threadId"] = serde_json::Value::String(tid.to_string());
    }

    match ureq::post("https://gmail.googleapis.com/gmail/v1/users/me/messages/send")
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Gmail API send failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Push draft to Gmail Drafts via Gmail API (drafts.create endpoint).
///
/// Uses OAuth2 token from the token store (scope: gmail.send).
/// The email is serialized as RFC 2822, base64url-encoded, and POSTed to the drafts endpoint.
fn push_to_drafts_via_gmail_api(
    email: &Message,
    account_name: &str,
    user: &str,
    thread_id: Option<&str>,
) -> Result<()> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let token = crate::filter::gmail_auth::get_send_access_token(Some(account_name), Some(user))?;

    let raw_bytes = email.formatted();
    let encoded = URL_SAFE_NO_PAD.encode(&raw_bytes);

    let mut msg_json = serde_json::json!({ "raw": encoded });
    if let Some(tid) = thread_id {
        msg_json["threadId"] = serde_json::Value::String(tid.to_string());
    }
    let body = serde_json::json!({
        "message": msg_json
    });

    match ureq::post("https://gmail.googleapis.com/gmail/v1/users/me/drafts")
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Gmail API draft push failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Send email via SMTP.
fn send_email(
    email: &Message,
    smtp_host: &str,
    smtp_port: u16,
    user: &str,
    password: &str,
) -> Result<()> {
    let creds = Credentials::new(user.to_string(), password.to_string());
    let mailer = SmtpTransport::relay(smtp_host)?
        .port(smtp_port)
        .credentials(creds)
        .build();
    mailer.send(email)?;
    Ok(())
}

/// Update the status field in a draft file (supports both YAML and legacy formats).
fn update_draft_status(path: &Path, new_status: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)?;

    if is_yaml_format(&text) {
        let after_first = &text[4..]; // skip "---\n"
        let end = after_first.find("\n---").ok_or_else(|| {
            anyhow::anyhow!("Missing closing YAML frontmatter delimiter")
        })?;
        let yaml_str = &after_first[..end];
        let rest = &after_first[end..]; // includes "\n---" and body

        let mut meta: EmailDraftMeta = serde_yaml::from_str(yaml_str)?;
        meta.status = new_status.to_string();
        let new_yaml = serde_yaml::to_string(&meta)?;
        let updated = format!("---\n{}{}", new_yaml, rest);
        std::fs::write(path, updated)?;
    } else {
        let re = Regex::new(r"(?m)^(\*\*Status\*\*:\s*).+$")?;
        let updated = re
            .replace(&text, format!("${{1}}{}", new_status))
            .to_string();
        std::fs::write(path, updated)?;
    }

    Ok(())
}

/// Resolve password, returning empty string if unavailable (e.g., gmail-api provider).
fn resolve_password_optional(acct: &crate::accounts::Account) -> String {
    if acct.provider == "gmail-api" {
        // Gmail API accounts use OAuth tokens, not passwords
        return String::new();
    }
    resolve_password(acct).unwrap_or_default()
}

/// Resolve sending account from draft metadata.
///
/// Supports credential bubbling: if the draft lives inside a `mailboxes/` subtree,
/// walk parent directories upward looking for `.corky.toml` files with matching
/// account credentials. First match wins.
fn resolve_account(
    meta: &HashMap<String, String>,
    draft_path: &Path,
) -> Result<(String, crate::accounts::Account, String)> {
    // Try local accounts first (from resolved .corky.toml)
    let accounts = load_accounts(None)?;

    // Try **Account** field first
    if let Some(acct_name) = meta.get("Account")
        && !acct_name.is_empty()
            && let Some(acct) = accounts.get(acct_name) {
                let pwd = resolve_password_optional(acct);
                return Ok((acct_name.clone(), acct.clone(), pwd));
            }

    // Try **From** field to match by email
    if let Some(from_addr) = meta.get("From")
        && !from_addr.is_empty()
            && let Some((name, acct)) = get_account_for_email(&accounts, from_addr) {
                let pwd = resolve_password_optional(&acct);
                return Ok((name, acct, pwd));
            }

    // Fall back to default from local config
    if let Ok((name, acct)) = get_default_account(&accounts) {
        let pwd = resolve_password_optional(&acct);
        return Ok((name, acct, pwd));
    }

    // Credential bubbling: walk parent directories for .corky.toml with matching account
    if let Some(result) = bubble_credentials(meta, draft_path) {
        return Ok(result);
    }

    bail!("No account found for draft. Check .corky.toml or add **Account**/**From** to the draft.")
}

/// Walk parent directories from the draft's location, looking for `.corky.toml`
/// files with account credentials matching the From address.
fn bubble_credentials(
    meta: &HashMap<String, String>,
    draft_path: &Path,
) -> Option<(String, crate::accounts::Account, String)> {
    let from_addr = meta.get("From").filter(|s| !s.is_empty())?;

    // Start from the draft's parent directory and walk up
    let mut dir = draft_path.parent()?;
    loop {
        dir = dir.parent()?;
        let config_path = dir.join(".corky.toml");
        if config_path.exists()
            && let Ok(parent_accounts) = load_accounts(Some(&config_path))
                && let Some((name, acct)) = get_account_for_email(&parent_accounts, from_addr) {
                        let pwd = resolve_password_optional(&acct);
                        return Some((name, acct, pwd));
                    }
        // Stop at filesystem root
        if dir.parent().is_none() || dir == dir.parent().unwrap() {
            break;
        }
    }
    None
}

/// Resolve a media path: expand `~`, resolve relative paths against a base directory.
fn resolve_media_path(path_str: &str, base_dir: &Path) -> String {
    if path_str.starts_with("~/") {
        crate::resolve::expand_tilde(path_str).to_string_lossy().to_string()
    } else {
        let p = Path::new(path_str);
        if p.is_absolute() {
            path_str.to_string()
        } else {
            base_dir.join(path_str).to_string_lossy().to_string()
        }
    }
}

/// corky push-draft FILE [--send]
pub fn run(file: &Path, send: bool) -> Result<()> {
    if !file.exists() {
        bail!("File not found: {}", file.display());
    }

    let text = std::fs::read_to_string(file)?;
    let yaml_meta = if is_yaml_format(&text) {
        parse_draft_yaml(&text)
    } else {
        None
    };
    let attachments = yaml_meta.as_ref().map(|m| m.attachments.clone()).unwrap_or_default();
    let images = yaml_meta.as_ref().map(|m| m.images.clone()).unwrap_or_default();
    let draft_thread_id = yaml_meta.as_ref().and_then(|m| m.thread_id.clone());

    // Resolve image paths relative to the draft file directory
    let draft_dir = file.parent().unwrap_or(Path::new("."));
    let resolved_images: Vec<String> = images
        .iter()
        .map(|p| resolve_media_path(p, draft_dir))
        .collect();

    let (meta, subject, body) = parse_draft(file)?;

    // Validate Status for --send
    let status = meta
        .get("Status")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    if send && !status.is_empty() && !VALID_SEND_STATUSES.contains(&status.as_str()) {
        bail!(
            "Cannot send: Status is '{}'. Must be one of: {}",
            meta.get("Status").unwrap_or(&String::new()),
            VALID_SEND_STATUSES.join(", ")
        );
    }

    let (acct_name, acct, password) = resolve_account(&meta, file)?;
    let use_gmail_api = acct.provider == "gmail-api";

    println!("Account: {} ({}){}", acct_name, acct.user, if use_gmail_api { " [Gmail API]" } else { "" });
    println!("To:      {}", meta["To"]);
    println!("Subject: {}", subject);
    if let Some(author) = meta.get("Author") {
        println!("Author:  {}", author);
    }
    if let Some(status) = meta.get("Status") {
        println!("Status:  {}", status);
    }
    if let Some(reply_to) = meta.get("In-Reply-To") {
        println!("Reply:   {}", reply_to);
    }
    if !attachments.is_empty() {
        println!("Attach:  {} file(s)", attachments.len());
        for a in &attachments {
            println!("         {}", a);
        }
    }
    if !resolved_images.is_empty() {
        println!("Images:  {} file(s)", resolved_images.len());
        for img in &resolved_images {
            println!("         {}", img);
        }
    }
    let body_preview = if body.len() > 80 {
        format!("{}...", &body[..80])
    } else {
        body.clone()
    };
    println!("Body:    {}", body_preview);
    println!();

    let email = compose_email(&meta, &subject, &body, &acct.user, &attachments, &resolved_images)?;

    if send {
        if use_gmail_api {
            send_via_gmail_api(&email, &acct_name, &acct.user, draft_thread_id.as_deref())?;
        } else {
            send_email(&email, &acct.smtp_host, acct.smtp_port, &acct.user, &password)?;
        }
        update_draft_status(file, "sent")?;
        println!("Email sent. Status updated to 'sent'.");
    } else {
        if use_gmail_api {
            push_to_drafts_via_gmail_api(&email, &acct_name, &acct.user, draft_thread_id.as_deref())?;
        } else {
            push_to_drafts(
                &email,
                &acct.imap_host,
                acct.imap_port,
                acct.imap_starttls,
                &acct.user,
                &password,
                &acct.drafts_folder,
            )?;
        }
        println!("Draft created. Open your email drafts to review and send.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn yaml_draft_content() -> String {
        "---\nto: alice@example.com\ncc: bob@example.com\nstatus: draft\nauthor: Brian\naccount: personal\nfrom: brian@example.com\nin_reply_to: \"<msg-1>\"\n---\n\n# Test Subject\n\nHello, this is the body.\n".to_string()
    }

    fn legacy_draft_content() -> String {
        "# Test Subject\n\n**To**: alice@example.com\n**CC**: bob@example.com\n**Status**: draft\n**Author**: Brian\n**Account**: personal\n**From**: brian@example.com\n**In-Reply-To**: <msg-1>\n\n---\n\nHello, this is the body.\n".to_string()
    }

    #[test]
    fn test_yaml_parse_roundtrip() {
        let content = yaml_draft_content();
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        let (meta, subject, body) = parse_draft(tmp.path()).unwrap();
        assert_eq!(meta["To"], "alice@example.com");
        assert_eq!(meta["CC"], "bob@example.com");
        assert_eq!(meta["Status"], "draft");
        assert_eq!(meta["Author"], "Brian");
        assert_eq!(meta["Account"], "personal");
        assert_eq!(meta["From"], "brian@example.com");
        assert_eq!(meta["In-Reply-To"], "<msg-1>");
        assert_eq!(subject, "Test Subject");
        assert!(body.contains("Hello, this is the body."));
    }

    #[test]
    fn test_legacy_parse_still_works() {
        let content = legacy_draft_content();
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        let (meta, subject, body) = parse_draft(tmp.path()).unwrap();
        assert_eq!(meta["To"], "alice@example.com");
        assert_eq!(meta["CC"], "bob@example.com");
        assert_eq!(meta["Status"], "draft");
        assert_eq!(meta["Author"], "Brian");
        assert_eq!(subject, "Test Subject");
        assert!(body.contains("Hello, this is the body."));
    }

    #[test]
    fn test_dual_format_detection() {
        assert!(is_yaml_format("---\nto: alice@example.com\n---\n"));
        assert!(!is_yaml_format("# Subject\n**To**: alice@example.com\n"));
    }

    #[test]
    fn test_yaml_status_update() {
        let content = yaml_draft_content();
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        update_draft_status(tmp.path(), "sent").unwrap();

        let updated = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(updated.contains("status: sent"));
        assert!(!updated.contains("status: draft"));
        // Body should still be there
        assert!(updated.contains("# Test Subject"));
    }

    #[test]
    fn test_legacy_status_update() {
        let content = legacy_draft_content();
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        update_draft_status(tmp.path(), "sent").unwrap();

        let updated = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(updated.contains("**Status**: sent"));
        assert!(!updated.contains("**Status**: draft"));
    }

    #[test]
    fn test_parse_draft_yaml_typed() {
        let content = yaml_draft_content();
        let meta = parse_draft_yaml(&content).unwrap();
        assert_eq!(meta.to, "alice@example.com");
        assert_eq!(meta.cc.as_deref(), Some("bob@example.com"));
        assert_eq!(meta.status, "draft");
        assert_eq!(meta.author.as_deref(), Some("Brian"));
        assert_eq!(meta.account.as_deref(), Some("personal"));
        assert_eq!(meta.from.as_deref(), Some("brian@example.com"));
        assert_eq!(meta.in_reply_to.as_deref(), Some("<msg-1>"));
    }

    #[test]
    fn test_parse_draft_yaml_returns_none_for_legacy() {
        let content = legacy_draft_content();
        assert!(parse_draft_yaml(&content).is_none());
    }

    #[test]
    fn test_yaml_minimal() {
        let content = "---\nto: alice@example.com\n---\n\n# Hello\n\nBody here\n";
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        let (meta, subject, body) = parse_draft(tmp.path()).unwrap();
        assert_eq!(meta["To"], "alice@example.com");
        assert_eq!(meta["Status"], "draft"); // default
        assert_eq!(subject, "Hello");
        assert!(body.contains("Body here"));
    }

    #[test]
    fn test_markdown_to_html_basic() {
        let html = markdown_to_html("Hello **world**");
        assert!(html.contains("<strong>world</strong>"));
        assert!(html.contains("<p>"));
    }

    #[test]
    fn test_markdown_to_html_links_and_lists() {
        let md = "Check [this](https://example.com)\n\n- item one\n- item two\n";
        let html = markdown_to_html(md);
        assert!(html.contains("<a href=\"https://example.com\">this</a>"));
        assert!(html.contains("<li>"));
    }

    #[test]
    fn test_compose_email_contains_html() {
        let mut meta = HashMap::new();
        meta.insert("To".to_string(), "alice@example.com".to_string());
        let email = compose_email(&meta, "Test", "Hello **world**", "bob@example.com", &[], &[]).unwrap();
        let bytes = email.formatted();
        let formatted = String::from_utf8_lossy(&bytes);
        assert!(formatted.contains("multipart/alternative"), "should be multipart/alternative");
        assert!(formatted.contains("text/plain"), "should contain text/plain part");
        assert!(formatted.contains("text/html"), "should contain text/html part");
        assert!(formatted.contains("<strong>world</strong>"), "should contain rendered HTML");
    }

    #[test]
    fn test_compose_email_with_attachment_contains_html() {
        let mut meta = HashMap::new();
        meta.insert("To".to_string(), "alice@example.com".to_string());
        // Create a temp attachment file
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "file contents").unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        let email = compose_email(&meta, "Test", "Hello **world**", "bob@example.com", &[path], &[]).unwrap();
        let bytes = email.formatted();
        let formatted = String::from_utf8_lossy(&bytes);
        assert!(formatted.contains("multipart/mixed"), "should be multipart/mixed with attachments");
        assert!(formatted.contains("multipart/alternative"), "should contain nested alternative");
        assert!(formatted.contains("text/plain"), "should contain text/plain part");
        assert!(formatted.contains("text/html"), "should contain text/html part");
        assert!(formatted.contains("<strong>world</strong>"), "should contain rendered HTML");
    }

    #[test]
    fn test_compose_email_with_inline_image() {
        let mut meta = HashMap::new();
        meta.insert("To".to_string(), "alice@example.com".to_string());
        // Create a temp image file (fake PNG)
        let mut tmp = NamedTempFile::with_suffix(".png").unwrap();
        write!(tmp, "fake png data").unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        let email = compose_email(&meta, "Test", "Hello", "bob@example.com", &[], &[path]).unwrap();
        let bytes = email.formatted();
        let formatted = String::from_utf8_lossy(&bytes);
        assert!(formatted.contains("multipart/related"), "should be multipart/related with inline images");
        assert!(formatted.contains("cid:image1@corky"), "should contain CID reference in HTML");
        assert!(formatted.contains("Content-Disposition: inline"), "should have inline disposition");
    }

    #[test]
    fn test_compose_email_with_both_attachments_and_images() {
        let mut meta = HashMap::new();
        meta.insert("To".to_string(), "alice@example.com".to_string());

        let mut img = NamedTempFile::with_suffix(".png").unwrap();
        write!(img, "fake png").unwrap();
        let img_path = img.path().to_string_lossy().to_string();

        let mut att = NamedTempFile::with_suffix(".pdf").unwrap();
        write!(att, "fake pdf").unwrap();
        let att_path = att.path().to_string_lossy().to_string();

        let email = compose_email(&meta, "Test", "Hello", "bob@example.com", &[att_path], &[img_path]).unwrap();
        let bytes = email.formatted();
        let formatted = String::from_utf8_lossy(&bytes);
        assert!(formatted.contains("multipart/mixed"), "should be multipart/mixed as outer");
        assert!(formatted.contains("multipart/related"), "should contain multipart/related for images");
    }

    #[test]
    fn test_resolve_media_path_absolute() {
        let result = resolve_media_path("/tmp/image.png", Path::new("/drafts"));
        assert_eq!(result, "/tmp/image.png");
    }

    #[test]
    fn test_resolve_media_path_relative() {
        let result = resolve_media_path("image.png", Path::new("/drafts"));
        assert_eq!(result, "/drafts/image.png");
    }

    #[test]
    fn test_resolve_media_path_tilde() {
        let result = resolve_media_path("~/images/photo.png", Path::new("/drafts"));
        let home = std::env::var("HOME").unwrap();
        assert_eq!(result, format!("{}/images/photo.png", home));
    }

    #[test]
    fn test_parse_draft_yaml_with_images() {
        let content = "---\nto: alice@example.com\nstatus: draft\nimages:\n  - screenshot.png\n  - photo.jpg\n---\n\n# Test\n\nBody\n";
        let meta = parse_draft_yaml(content).unwrap();
        assert_eq!(meta.images.len(), 2);
        assert_eq!(meta.images[0], "screenshot.png");
        assert_eq!(meta.images[1], "photo.jpg");
    }
}
