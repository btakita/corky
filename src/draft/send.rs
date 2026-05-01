//! `corky draft send` — send a draft via the Gmail API with optional attachments.
//!
//! Unlike `corky draft push --send` (SMTP/lettre), this path:
//! - Uses the Gmail REST API directly (no SMTP credentials needed)
//! - Supports file attachments via MIME multipart/mixed
//! - Handles reply threading (In-Reply-To + threadId)

use anyhow::{Result, bail};
use base64::Engine as _;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::draft::{parse_draft, parse_draft_yaml};
use crate::filter::gmail_auth;

const GMAIL_SEND_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages/send";

#[derive(Debug, Clone, Serialize)]
pub struct DraftSendResult {
    pub action: String,
    pub transport: String,
    pub account_key: String,
    pub account_hint: Option<String>,
    pub to: String,
    pub subject: String,
    pub in_reply_to: Option<String>,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub attachment_paths: Vec<String>,
}

/// Send a draft file via the Gmail API.
///
/// `extra_attachments` are paths given on the CLI; the draft's own `attachments`
/// field is also included.
pub fn run(file: &Path, extra_attachments: &[PathBuf], account: Option<&str>) -> Result<()> {
    run_internal(file, extra_attachments, account, false).map(|_| ())
}

pub fn run_with_report(
    file: &Path,
    extra_attachments: &[PathBuf],
    account: Option<&str>,
) -> Result<DraftSendResult> {
    run_internal(file, extra_attachments, account, true)
}

fn run_internal(
    file: &Path,
    extra_attachments: &[PathBuf],
    account: Option<&str>,
    quiet: bool,
) -> Result<DraftSendResult> {
    let content = std::fs::read_to_string(file)?;
    let (draft_meta, subject, body) = parse_draft(file)?;
    let (account_name, acct, _password) = super::resolve_account(&draft_meta, file)?;

    let meta = parse_draft_yaml(&content).ok_or_else(|| {
        anyhow::anyhow!(
            "Draft must use YAML frontmatter format. Run `corky draft migrate` to convert."
        )
    })?;

    // Collect attachments: draft field + CLI extras
    let mut attachment_paths: Vec<PathBuf> = meta.attachments.iter().map(PathBuf::from).collect();
    attachment_paths.extend_from_slice(extra_attachments);
    let attachment_strings: Vec<String> = attachment_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    let from = meta.from.as_deref().unwrap_or(&acct.user);
    let account_hint = account
        .map(str::to_string)
        .or_else(|| Some(acct.user.clone()));

    let token = gmail_auth::get_send_access_token(
        Some(&account_name),
        account.or(Some(acct.user.as_str())),
    )?;

    let mime = build_mime_message(
        &meta.to,
        from,
        &subject,
        &body,
        &meta.in_reply_to,
        &attachment_paths,
    )?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&mime);

    let mut payload = serde_json::json!({ "raw": raw });
    if let Some(tid) = &meta.thread_id {
        payload["threadId"] = serde_json::Value::String(tid.clone());
    }

    if !quiet {
        eprintln!("Sending to {} — subject: {}", meta.to, subject);
        if !attachment_paths.is_empty() {
            eprintln!("Attachments ({}):", attachment_paths.len());
            for p in &attachment_paths {
                eprintln!("  {}", p.display());
            }
        }
    }

    let resp = ureq::post(GMAIL_SEND_URL)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(&payload);

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.into_json()?;
            let msg_id = body["id"].as_str().unwrap_or("(unknown)");
            if !quiet {
                println!("Sent. Message ID: {}", msg_id);
            }
            Ok(DraftSendResult {
                action: "sent".to_string(),
                transport: "gmail-api".to_string(),
                account_key: account_name.clone(),
                account_hint,
                to: meta.to.clone(),
                subject,
                in_reply_to: meta.in_reply_to.clone(),
                thread_id: body["threadId"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| meta.thread_id.clone()),
                message_id: body["id"].as_str().map(str::to_string),
                attachment_paths: attachment_strings,
            })
        }
        Err(ureq::Error::Status(401, _)) => {
            let cleared = gmail_auth::clear_send_token(Some(&account_name));
            match cleared {
                Ok(true) => bail!(
                    "Gmail API: unauthorized (401). Cleared the cached gmail.compose send token; re-run `corky draft send` to re-authenticate."
                ),
                Ok(false) => bail!(
                    "Gmail API: unauthorized (401). Re-run `corky draft send` to re-authenticate the gmail.compose send token."
                ),
                Err(err) => bail!(
                    "Gmail API: unauthorized (401). Re-run `corky draft send` to re-authenticate the gmail.compose send token. Also failed to clear the cached send token: {}",
                    err
                ),
            }
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Gmail API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Build a RFC 2822 MIME message as bytes.
///
/// Returns `multipart/mixed` when attachments are present, otherwise `text/plain`.
fn build_mime_message(
    to: &str,
    from: &str,
    subject: &str,
    body: &str,
    in_reply_to: &Option<String>,
    attachments: &[PathBuf],
) -> Result<Vec<u8>> {
    let boundary = format!("corky_boundary_{}", chrono::Utc::now().timestamp_millis());
    let mut msg = String::new();

    if !from.is_empty() {
        msg.push_str(&format!("From: {}\r\n", from));
    }
    msg.push_str(&format!("To: {}\r\n", to));
    msg.push_str(&format!("Subject: {}\r\n", encode_header(subject)));
    if let Some(mid) = in_reply_to {
        msg.push_str(&format!("In-Reply-To: {}\r\n", mid));
        msg.push_str(&format!("References: {}\r\n", mid));
    }
    msg.push_str("MIME-Version: 1.0\r\n");

    if attachments.is_empty() {
        msg.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
        msg.push_str("\r\n");
        msg.push_str(body);
    } else {
        msg.push_str(&format!(
            "Content-Type: multipart/mixed; boundary=\"{}\"\r\n",
            boundary
        ));
        msg.push_str("\r\n");

        // Text part
        msg.push_str(&format!("--{}\r\n", boundary));
        msg.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
        msg.push_str("\r\n");
        msg.push_str(body);
        msg.push_str("\r\n");

        // Attachment parts
        for path in attachments {
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "attachment".to_string());
            let data = std::fs::read(path)?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
            let mime_type = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();

            msg.push_str(&format!("--{}\r\n", boundary));
            msg.push_str(&format!(
                "Content-Type: {}; name=\"{}\"\r\n",
                mime_type, filename
            ));
            msg.push_str("Content-Transfer-Encoding: base64\r\n");
            msg.push_str(&format!(
                "Content-Disposition: attachment; filename=\"{}\"\r\n",
                filename
            ));
            msg.push_str("\r\n");
            // Break base64 into 76-char lines (RFC 2045)
            for chunk in encoded.as_bytes().chunks(76) {
                msg.push_str(std::str::from_utf8(chunk)?);
                msg.push_str("\r\n");
            }
        }

        msg.push_str(&format!("--{}--\r\n", boundary));
    }

    Ok(msg.into_bytes())
}

/// Encode a header value using RFC 2047 base64 UTF-8 if it contains non-ASCII.
fn encode_header(value: &str) -> String {
    if value.is_ascii() {
        value.to_string()
    } else {
        let encoded = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
        format!("=?UTF-8?B?{}?=", encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_header_ascii() {
        assert_eq!(encode_header("Hello World"), "Hello World");
        assert_eq!(encode_header("Re: Meeting"), "Re: Meeting");
    }

    #[test]
    fn test_encode_header_utf8() {
        let encoded = encode_header("Héllo");
        assert!(encoded.starts_with("=?UTF-8?B?"));
        assert!(encoded.ends_with("?="));
    }

    #[test]
    fn test_build_mime_no_attachments() {
        let mime = build_mime_message(
            "alice@example.com",
            "brian@example.com",
            "Test Subject",
            "Hello Alice",
            &None,
            &[],
        )
        .unwrap();
        let text = String::from_utf8(mime).unwrap();
        assert!(text.contains("To: alice@example.com"));
        assert!(text.contains("Subject: Test Subject"));
        assert!(text.contains("Content-Type: text/plain"));
        assert!(text.contains("Hello Alice"));
        assert!(!text.contains("multipart"));
    }

    #[test]
    fn test_build_mime_with_in_reply_to() {
        let mime = build_mime_message(
            "alice@example.com",
            "",
            "Re: Test",
            "Body",
            &Some("<original@example.com>".to_string()),
            &[],
        )
        .unwrap();
        let text = String::from_utf8(mime).unwrap();
        assert!(text.contains("In-Reply-To: <original@example.com>"));
        assert!(text.contains("References: <original@example.com>"));
    }

    #[test]
    fn test_build_mime_with_attachment() {
        use std::io::Write;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.as_file().write_all(b"file content").unwrap();
        let path = tmp.path().to_path_buf();

        let mime = build_mime_message(
            "alice@example.com",
            "brian@example.com",
            "Subject",
            "Body",
            &None,
            &[path],
        )
        .unwrap();
        let text = String::from_utf8(mime).unwrap();
        assert!(text.contains("Content-Type: multipart/mixed"));
        assert!(text.contains("Content-Transfer-Encoding: base64"));
        assert!(text.contains("Content-Disposition: attachment"));
    }
}
