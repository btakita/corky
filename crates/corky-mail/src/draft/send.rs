//! `corky draft send` — send a draft via the Gmail API with optional attachments.
//!
//! Unlike `corky draft push --send` (SMTP/lettre), this path:
//! - Uses the Gmail REST API directly (no SMTP credentials needed)
//! - Supports file attachments via MIME multipart/mixed
//! - Handles reply threading (In-Reply-To + threadId)

use anyhow::{Result, bail};
use base64::Engine as _;
use serde::Serialize;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::draft::{parse_draft, parse_draft_yaml};
use crate::filter::gmail_auth;

const GMAIL_SEND_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages/send";

/// Attachments whose combined size meets this threshold are sent through the
/// streaming path (#ckymimestream): the MIME is spooled to a temp file and the
/// Gmail `raw` JSON body is produced by streaming a base64url encoder, so the
/// message is never held in RAM as a single ~1.3× string. Below the threshold
/// the simple in-memory path is used unchanged.
const LARGE_ATTACHMENT_BYTES: u64 = 5 * 1024 * 1024;

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

/// A reply needs BOTH `in_reply_to` (RFC 2822 Message-ID → the `In-Reply-To`
/// header) AND `thread_id` (Gmail's internal id → the API `threadId`) to thread.
/// If only one is set the reply silently un-threads; return a warning naming the
/// missing field (#ckythread).
fn reply_threading_warning(
    in_reply_to: &Option<String>,
    thread_id: &Option<String>,
) -> Option<String> {
    let has_irt = in_reply_to.as_deref().is_some_and(|s| !s.trim().is_empty());
    let has_tid = thread_id.as_deref().is_some_and(|s| !s.trim().is_empty());
    match (has_irt, has_tid) {
        (true, false) => Some(
            "draft sets `in_reply_to` but not `thread_id`; Gmail needs both to thread a reply, \
             so it will be sent as a new thread. Add the original thread's `thread_id`."
                .to_string(),
        ),
        (false, true) => Some(
            "draft sets `thread_id` but not `in_reply_to`; the `In-Reply-To` header will be \
             missing, so non-Gmail clients won't thread the reply. Add the original Message-ID \
             as `in_reply_to`."
                .to_string(),
        ),
        _ => None,
    }
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

    // #ckythread: warn loudly when a reply has only one of the two threading
    // fields, instead of silently un-threading.
    if let Some(w) = reply_threading_warning(&meta.in_reply_to, &meta.thread_id) {
        eprintln!("  warning: {w}");
    }

    // Collect attachments: draft field + CLI extras.
    // Draft-declared attachment paths follow the same convention as `images:` —
    // a bare/relative path is anchored to the draft file's directory, not the
    // process cwd — so resolve them before reading. CLI extras are taken as-is
    // (already resolved by the caller's shell).
    let draft_dir = file.parent().unwrap_or_else(|| Path::new("."));
    let mut attachment_paths: Vec<PathBuf> = meta
        .attachments
        .iter()
        .map(|p| PathBuf::from(super::resolve_media_path(p, draft_dir)))
        .collect();
    attachment_paths.extend_from_slice(extra_attachments);
    let attachment_strings: Vec<String> = attachment_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    // #ckymimeinline: resolve inline `images:` paths (same anchor convention as
    // `attachments:`) so draft send embeds them with CIDs instead of dropping them.
    let image_paths: Vec<PathBuf> = meta
        .images
        .iter()
        .map(|p| PathBuf::from(super::resolve_media_path(p, draft_dir)))
        .collect();

    let from = meta.from.as_deref().unwrap_or(&acct.user);
    let account_hint = account
        .map(str::to_string)
        .or_else(|| Some(acct.user.clone()));

    let token = gmail_auth::get_send_access_token(
        Some(&account_name),
        account.or(Some(acct.user.as_str())),
    )?;

    // #ckymimestream: for large attachments/images, spool the MIME to a temp file
    // and stream the base64url `raw` body so the message is never held in RAM as
    // one ~1.3× string. Below the threshold the simple in-memory JSON path is used.
    let total_media_bytes: u64 = attachment_paths
        .iter()
        .chain(image_paths.iter())
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();

    if !quiet {
        eprintln!("Sending to {} — subject: {}", meta.to, subject);
        if !attachment_paths.is_empty() {
            eprintln!("Attachments ({}):", attachment_paths.len());
            for p in &attachment_paths {
                eprintln!("  {}", p.display());
            }
        }
    }

    let resp_result = if total_media_bytes >= LARGE_ATTACHMENT_BYTES {
        // Spool streamed MIME to an unnamed temp file, then stream it back
        // through the base64url JSON body.
        let mut spool = tempfile::tempfile()?;
        build_mime_to_writer(
            &mut spool,
            &meta.to,
            from,
            &subject,
            &body,
            &meta.in_reply_to,
            &attachment_paths,
            &image_paths,
        )?;
        spool.seek(SeekFrom::Start(0))?;
        let body = StreamingRawBody::new(spool, meta.thread_id.as_deref());
        ureq::post(GMAIL_SEND_URL)
            .set("Authorization", &format!("Bearer {}", token))
            .set("Content-Type", "application/json")
            .send(body)
    } else {
        let mime = build_mime_message(
            &meta.to,
            from,
            &subject,
            &body,
            &meta.in_reply_to,
            &attachment_paths,
            &image_paths,
        )?;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&mime);
        let mut payload = serde_json::json!({ "raw": raw });
        if let Some(tid) = &meta.thread_id {
            payload["threadId"] = serde_json::Value::String(tid.clone());
        }
        ureq::post(GMAIL_SEND_URL)
            .set("Authorization", &format!("Bearer {}", token))
            .set("Content-Type", "application/json")
            .send_json(&payload)
    };

    match resp_result {
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
/// Sanitize a filename for a MIME `quoted-string` (#ckymime): drop control
/// characters (CR/LF would inject headers) and backslash-escape `"` and `\`.
fn sanitize_mime_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_control() {
            continue;
        }
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    if out.is_empty() {
        "attachment".to_string()
    } else {
        out
    }
}

/// Normalize a Message-ID to RFC 5322 angle-bracket form (#ckymime):
/// `abc@x` → `<abc@x>`; an already-bracketed id is returned unchanged.
fn normalize_message_id(mid: &str) -> String {
    let trimmed = mid.trim();
    if trimmed.starts_with('<') && trimmed.ends_with('>') && trimmed.len() >= 2 {
        trimmed.to_string()
    } else {
        format!("<{}>", trimmed.trim_matches(['<', '>']))
    }
}

/// Streaming base64url-no-pad encoder over a reader (#ckymimestream).
///
/// Reads raw bytes from `inner` and exposes them as base64url (no padding),
/// carrying 0–2 bytes across `read` boundaries so 3-byte groups align. At EOF
/// the trailing 1–2 bytes are encoded without padding. This lets the Gmail
/// `raw` body be produced incrementally instead of base64-encoding the whole
/// message into one string.
struct Base64UrlReader<R: Read> {
    inner: R,
    carry: Vec<u8>,
    out: Vec<u8>,
    out_pos: usize,
    eof: bool,
}

impl<R: Read> Base64UrlReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            carry: Vec::new(),
            out: Vec::new(),
            out_pos: 0,
            eof: false,
        }
    }

    /// Read up to `RAW_CHUNK * N` bytes, encode the 3-byte-aligned portion, and
    /// carry the remainder. Returns the number of raw bytes consumed.
    fn refill(&mut self) -> std::io::Result<usize> {
        const RAW_CHUNK: usize = 57 * 64; // multiple of 3 → clean 76-char lines
        let mut buf = vec![0u8; RAW_CHUNK];
        let mut total = 0;
        while total < RAW_CHUNK {
            let n = self.inner.read(&mut buf[total..])?;
            if n == 0 {
                break;
            }
            total += n;
        }
        if total == 0 {
            return Ok(0);
        }
        let mut combined = std::mem::take(&mut self.carry);
        combined.extend_from_slice(&buf[..total]);
        let aligned = (combined.len() / 3) * 3;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&combined[..aligned]);
        if aligned < combined.len() {
            self.carry = combined[aligned..].to_vec();
        }
        self.out = enc.into_bytes();
        self.out_pos = 0;
        Ok(total)
    }
}

impl<R: Read> Read for Base64UrlReader<R> {
    fn read(&mut self, dst: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.out_pos < self.out.len() {
                let to_copy = std::cmp::min(self.out.len() - self.out_pos, dst.len());
                dst[..to_copy].copy_from_slice(&self.out[self.out_pos..self.out_pos + to_copy]);
                self.out_pos += to_copy;
                return Ok(to_copy);
            }
            if self.eof {
                return Ok(0);
            }
            let consumed = self.refill()?;
            if consumed == 0 {
                self.eof = true;
                if !self.carry.is_empty() {
                    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.carry);
                    self.out = enc.into_bytes();
                    self.out_pos = 0;
                    self.carry.clear();
                    continue;
                }
                return Ok(0);
            }
        }
    }
}

/// Emits the Gmail `messages.send` JSON body `{"raw":"<base64url(mime)>"[,"threadId":"…"]}`
/// by streaming `mime` through a [`Base64UrlReader`], so a large message is
/// never materialized as a single string (#ckymimestream).
struct StreamingRawBody<R: Read> {
    phase: StreamingPhase,
    header: Cursor<Vec<u8>>,
    b64: Base64UrlReader<R>,
    tail: Cursor<Vec<u8>>,
}

enum StreamingPhase {
    Header,
    Base64,
    Tail,
    Done,
}

impl<R: Read> StreamingRawBody<R> {
    fn new(mime: R, thread_id: Option<&str>) -> Self {
        let tail = match thread_id.filter(|t| !t.trim().is_empty()) {
            // Close the `raw` string with a leading `"`, then the rest of the object.
            Some(t) => format!("\",\"threadId\":\"{}\"}}", t),
            None => "\"}".to_string(),
        };
        Self {
            phase: StreamingPhase::Header,
            header: Cursor::new(b"{\"raw\":\"".to_vec()),
            b64: Base64UrlReader::new(mime),
            tail: Cursor::new(tail.into_bytes()),
        }
    }
}

impl<R: Read> Read for StreamingRawBody<R> {
    fn read(&mut self, dst: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.phase {
                StreamingPhase::Header => {
                    let n = self.header.read(dst)?;
                    if n == 0 {
                        self.phase = StreamingPhase::Base64;
                        continue;
                    }
                    return Ok(n);
                }
                StreamingPhase::Base64 => {
                    let n = self.b64.read(dst)?;
                    if n == 0 {
                        self.phase = StreamingPhase::Tail;
                        continue;
                    }
                    return Ok(n);
                }
                StreamingPhase::Tail => {
                    let n = self.tail.read(dst)?;
                    if n == 0 {
                        self.phase = StreamingPhase::Done;
                        continue;
                    }
                    return Ok(n);
                }
                StreamingPhase::Done => return Ok(0),
            }
        }
    }
}

/// Build a RFC 2822 MIME message as bytes.
///
/// - No images/attachments → `text/plain`.
/// - Attachments only → `multipart/mixed`.
/// - Inline images → `multipart/related` (and `multipart/mixed` around it when
///   attachments are also present), mirroring `draft push` so `images:` YAML is
///   honored instead of silently dropped (#ckymimeinline).
pub fn build_mime_message(
    to: &str,
    from: &str,
    subject: &str,
    body: &str,
    in_reply_to: &Option<String>,
    attachments: &[PathBuf],
    images: &[PathBuf],
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    build_mime_to_writer(&mut buf, to, from, subject, body, in_reply_to, attachments, images)?;
    Ok(buf)
}

/// Stream a file's contents as base64 in 57-byte (→ 76-char) lines (#ckymimestream).
fn write_base64_stream<W: Write>(out: &mut W, path: &Path) -> Result<()> {
    let mut f = std::fs::File::open(path)?;
    const RAW_CHUNK: usize = 57;
    let mut buf = [0u8; RAW_CHUNK];
    loop {
        let mut filled = 0;
        while filled < RAW_CHUNK {
            let n = f.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&buf[..filled]);
        out.write_all(encoded.as_bytes())?;
        write!(out, "\r\n")?;
        if filled < RAW_CHUNK {
            break;
        }
    }
    Ok(())
}

/// Build a RFC 2822 MIME message streamed into `out` (#ckymimestream, #ckymimeinline).
///
/// Streaming variant of [`build_mime_message`]: attachments and inline images
/// are read and base64-encoded in 57-byte (→ 76-char) chunks written straight to
/// `out`, so neither the raw bytes nor their full base64 string are held in
/// memory at once.
#[allow(clippy::too_many_arguments)]
pub fn build_mime_to_writer<W: Write>(
    out: &mut W,
    to: &str,
    from: &str,
    subject: &str,
    body: &str,
    in_reply_to: &Option<String>,
    attachments: &[PathBuf],
    images: &[PathBuf],
) -> Result<()> {
    let ts = chrono::Utc::now().timestamp_millis();
    let boundary = format!("corky_boundary_{}", ts);

    if !from.is_empty() {
        write!(out, "From: {}\r\n", from)?;
    }
    write!(out, "To: {}\r\n", to)?;
    write!(out, "Subject: {}\r\n", encode_header(subject))?;
    if let Some(mid) = in_reply_to {
        // #ckymime: RFC 5322 msg-id must be angle-bracketed; a bare id is malformed.
        let mid = normalize_message_id(mid);
        write!(out, "In-Reply-To: {}\r\n", mid)?;
        write!(out, "References: {}\r\n", mid)?;
    }
    write!(out, "MIME-Version: 1.0\r\n")?;

    let has_images = !images.is_empty();
    let has_attachments = !attachments.is_empty();

    // Simplest case: plain text only.
    if !has_images && !has_attachments {
        write!(out, "Content-Type: text/plain; charset=UTF-8\r\n\r\n{}", body)?;
        return Ok(());
    }

    // Attachments only: multipart/mixed (text/plain + attachments).
    if !has_images {
        write!(
            out,
            "Content-Type: multipart/mixed; boundary=\"{}\"\r\n\r\n",
            boundary
        )?;
        write!(
            out,
            "--{}\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{}\r\n",
            boundary, body
        )?;
        for path in attachments {
            write_part(out, &boundary, path, PartKind::Attachment)?;
        }
        write!(out, "--{}--\r\n", boundary)?;
        return Ok(());
    }

    // Inline images: multipart/related around the body. CIDs use the same
    // `image{N}@corky` convention as `draft push` so the two paths agree.
    let related_b = format!("corky_rel_{}", ts);
    let alt_b = format!("corky_alt_{}", ts);
    let cids: Vec<String> = images
        .iter()
        .enumerate()
        .map(|(i, _)| format!("image{}@corky", i + 1))
        .collect();

    if has_attachments {
        // Outer multipart/mixed, first part is the related block.
        write!(
            out,
            "Content-Type: multipart/mixed; boundary=\"{}\"\r\n\r\n",
            boundary
        )?;
        write!(out, "--{}\r\n", boundary)?;
        write!(
            out,
            "Content-Type: multipart/related; boundary=\"{}\"; type=\"multipart/alternative\"\r\n\r\n",
            related_b
        )?;
    } else {
        write!(
            out,
            "Content-Type: multipart/related; boundary=\"{}\"; type=\"multipart/alternative\"\r\n\r\n",
            related_b
        )?;
    }

    // multipart/alternative (plain + html-with-cid-refs) inside the related block.
    write!(out, "--{}\r\n", related_b)?;
    write!(
        out,
        "Content-Type: multipart/alternative; boundary=\"{}\"\r\n\r\n",
        alt_b
    )?;
    write!(
        out,
        "--{}\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{}\r\n",
        alt_b, body
    )?;
    // HTML body, mirroring draft push: convert the markdown body and append
    // <img src="cid:…"> references for each inline image (#ckymimeinline).
    let mut html = super::markdown_to_html(body);
    for (img, cid) in images.iter().zip(cids.iter()) {
        let filename = img
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".to_string());
        html.push_str(&format!(
            "<p><img src=\"cid:{}\" alt=\"{}\" /></p>\n",
            cid, filename
        ));
    }
    write!(
        out,
        "--{}\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n{}\r\n",
        alt_b, html
    )?;
    write!(out, "--{}--\r\n", alt_b)?;

    // Inline image parts (Content-ID + inline disposition).
    for (img, cid) in images.iter().zip(cids.iter()) {
        write_inline_image(out, &related_b, img, cid)?;
    }
    write!(out, "--{}--\r\n", related_b)?;

    // Attachments in the outer mixed wrapper, if any.
    if has_attachments {
        for path in attachments {
            write_part(out, &boundary, path, PartKind::Attachment)?;
        }
        write!(out, "--{}--\r\n", boundary)?;
    }
    Ok(())
}

enum PartKind {
    Attachment,
}

/// Write a base64 attachment part under `boundary` (#ckymimestream).
fn write_part<W: Write>(
    out: &mut W,
    boundary: &str,
    path: &Path,
    _kind: PartKind,
) -> Result<()> {
    let raw_filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".to_string());
    // #ckymime: sanitize for the MIME quoted-string (no header injection).
    let filename = sanitize_mime_filename(&raw_filename);
    let mime_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    write!(out, "--{}\r\n", boundary)?;
    write!(
        out,
        "Content-Type: {}; name=\"{}\"\r\n",
        mime_type, filename
    )?;
    write!(out, "Content-Transfer-Encoding: base64\r\n")?;
    write!(
        out,
        "Content-Disposition: attachment; filename=\"{}\"\r\n\r\n",
        filename
    )?;
    write_base64_stream(out, path)
}

/// Write an inline image part with a `Content-ID` under `boundary` (#ckymimeinline).
fn write_inline_image<W: Write>(
    out: &mut W,
    boundary: &str,
    path: &Path,
    cid: &str,
) -> Result<()> {
    let raw_filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_string());
    let filename = sanitize_mime_filename(&raw_filename);
    let mime_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    write!(out, "--{}\r\n", boundary)?;
    write!(
        out,
        "Content-Type: {}; name=\"{}\"\r\n",
        mime_type, filename
    )?;
    write!(out, "Content-Transfer-Encoding: base64\r\n")?;
    write!(out, "Content-ID: <{}>\r\n", cid)?;
    write!(
        out,
        "Content-Disposition: inline; filename=\"{}\"\r\n\r\n",
        filename
    )?;
    write_base64_stream(out, path)
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
    fn sanitize_mime_filename_blocks_injection() {
        // #ckymime: CR/LF stripped, quotes/backslashes escaped.
        assert_eq!(sanitize_mime_filename("report.pdf"), "report.pdf");
        assert_eq!(
            sanitize_mime_filename("a\"b.txt"),
            "a\\\"b.txt"
        );
        // CR/LF removed → no header injection (space after `Bcc:` preserved).
        assert_eq!(
            sanitize_mime_filename("evil\r\nBcc: x@y.txt"),
            "evilBcc: x@y.txt"
        );
        assert_eq!(sanitize_mime_filename("back\\slash"), "back\\\\slash");
        // Control chars stripped entirely; empty result falls back.
        assert_eq!(sanitize_mime_filename("\r\n"), "attachment");
        // Unicode filenames pass through.
        assert_eq!(sanitize_mime_filename("报告.pdf"), "报告.pdf");
    }

    #[test]
    fn normalize_message_id_adds_angle_brackets() {
        // #ckymime: RFC 5322 angle-bracket form.
        assert_eq!(normalize_message_id("abc@mail"), "<abc@mail>");
        assert_eq!(normalize_message_id("<abc@mail>"), "<abc@mail>");
        assert_eq!(normalize_message_id("  <abc@mail>  "), "<abc@mail>");
        assert_eq!(normalize_message_id("<abc@mail"), "<abc@mail>");
    }

    #[test]
    fn reply_threading_warning_flags_partial_threading() {
        let s = |v: &str| Some(v.to_string());
        // Both present → no warning.
        assert!(reply_threading_warning(&s("<m@x>"), &s("t123")).is_none());
        // Neither present → not a reply, no warning.
        assert!(reply_threading_warning(&None, &None).is_none());
        // Only in_reply_to → warn about missing thread_id (Gmail).
        let w = reply_threading_warning(&s("<m@x>"), &None).unwrap();
        assert!(w.contains("thread_id"), "got: {w}");
        // Only thread_id → warn about missing in_reply_to (In-Reply-To header).
        let w = reply_threading_warning(&None, &s("t123")).unwrap();
        assert!(w.contains("in_reply_to"), "got: {w}");
        // Whitespace-only fields are treated as absent.
        assert!(reply_threading_warning(&s("  "), &s("  ")).is_none());
        assert!(reply_threading_warning(&s("<m@x>"), &s("  ")).unwrap().contains("thread_id"));
    }

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
            &[],
        )
        .unwrap();
        let text = String::from_utf8(mime).unwrap();
        assert!(text.contains("Content-Type: multipart/mixed"));
        assert!(text.contains("Content-Transfer-Encoding: base64"));
        assert!(text.contains("Content-Disposition: attachment"));
    }

    #[test]
    fn test_build_mime_to_writer_streams_attachment_roundtrip() {
        // #ckymimestream: streamed build must yield MIME whose base64 decodes
        // back to the original attachment bytes, with ≤76-char lines.
        use std::io::Write;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let payload = b"\xE2\x9C\x93 binary \x00\x01\x02 data with non-ascii"; // 36 bytes
        tmp.as_file().write_all(payload).unwrap();
        let path = tmp.path().to_path_buf();

        let mut out = Vec::new();
        build_mime_to_writer(
            &mut out,
            "alice@example.com",
            "brian@example.com",
            "Subj",
            "Body",
            &None,
            &[path],
            &[],
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Content-Type: multipart/mixed"));

        // Every base64 content line (between the attachment headers and the
        // closing boundary) must be ≤76 chars and decode back to the payload.
        let mut in_attachment = false;
        let mut collected = String::new();
        for line in text.lines() {
            if line.starts_with("--corky_boundary_") && line.ends_with("--") {
                in_attachment = false;
            } else if line.starts_with("Content-Disposition: attachment") {
                in_attachment = true;
            } else if in_attachment && !line.contains(':') && !line.is_empty() {
                assert!(line.len() <= 76, "base64 line too long: {}", line.len());
                collected.push_str(line);
            }
        }
        let decoded =
            base64::engine::general_purpose::STANDARD.decode(collected.as_bytes()).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn test_base64url_reader_matches_bulk_encode() {
        // #ckymimestream: the streaming base64url reader must produce exactly
        // what a one-shot URL_SAFE_NO_PAD encode produces, across sizes that
        // stress 3-byte-group alignment (carry of 0, 1, and 2 bytes).
        for size in [0usize, 1, 2, 3, 4, 5, 57, 58, 114, 115, 1000, 4096, 8192] {
            let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&data);

            let mut reader = Base64UrlReader::new(std::io::Cursor::new(data));
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();

            assert_eq!(
                String::from_utf8(got).unwrap(),
                expected,
                "base64url stream mismatch at size {size}"
            );
        }
    }

    #[test]
    fn test_streaming_raw_body_no_threadid() {
        let mime = b"From: a@b\r\n\r\nhello";
        let mut body = StreamingRawBody::new(std::io::Cursor::new(mime.to_vec()), None);
        let mut out = Vec::new();
        body.read_to_end(&mut out).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let raw = parsed["raw"].as_str().unwrap();
        let expected_raw =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mime);
        assert_eq!(raw, expected_raw);
        assert!(parsed.get("threadId").is_none(), "no threadId expected");
    }

    #[test]
    fn test_streaming_raw_body_with_threadid() {
        let mime = b"From: a@b\r\n\r\nhello";
        let mut body =
            StreamingRawBody::new(std::io::Cursor::new(mime.to_vec()), Some("t987"));
        let mut out = Vec::new();
        body.read_to_end(&mut out).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["raw"].as_str().unwrap(),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mime)
        );
        assert_eq!(parsed["threadId"].as_str().unwrap(), "t987");
    }

    #[test]
    fn test_build_mime_with_inline_image() {
        // #ckymimeinline: `images:` must be embedded as multipart/related with
        // Content-ID + inline disposition (previously dropped by draft send).
        use std::io::Write;
        let img = tempfile::NamedTempFile::new().unwrap();
        // Minimal 1x1 PNG.
        let png = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01,
        ];
        img.as_file().write_all(&png).unwrap();
        let img_path = img.path().to_path_buf();

        let mime = build_mime_message(
            "alice@example.com",
            "brian@example.com",
            "Subject",
            "Body",
            &None,
            &[],
            std::slice::from_ref(&img_path),
        )
        .unwrap();
        let text = String::from_utf8(mime).unwrap();

        // Wrapped in multipart/related with an inline image carrying a CID.
        assert!(text.contains("Content-Type: multipart/related"));
        assert!(text.contains("Content-Type: multipart/alternative"));
        assert!(text.contains("Content-ID: <image1@corky>"));
        assert!(text.contains("Content-Disposition: inline"));
        // The HTML alternative references the CID (mirrors draft push).
        assert!(text.contains("cid:image1@corky"));
        // The inline image bytes round-trip: collect base64 lines (the header
        // lines carry ':' and blank lines separate them from the payload) and
        // decode back to the original PNG.
        let in_image = text
            .lines()
            .position(|l| l.contains("Content-ID: <image1@corky>"))
            .unwrap();
        let mut collected = String::new();
        for line in text.lines().skip(in_image) {
            if line.starts_with("--") {
                break;
            }
            // base64 content lines have no ':' and aren't blank.
            if !line.is_empty() && !line.contains(':') {
                collected.push_str(line.trim());
            }
        }
        let decoded =
            base64::engine::general_purpose::STANDARD.decode(collected.as_bytes()).unwrap();
        assert_eq!(decoded, png);
    }

    #[test]
    fn test_build_mime_with_inline_image_and_attachment() {
        // #ckymimeinline: both images and attachments → multipart/mixed wraps a
        // multipart/related (alternative + inline image) plus the attachment.
        use std::io::Write;
        let img = tempfile::NamedTempFile::new().unwrap();
        img.as_file().write_all(b"imgdata").unwrap();
        let img_path = img.path().to_path_buf();
        let att = tempfile::NamedTempFile::new().unwrap();
        att.as_file().write_all(b"attdata").unwrap();
        let att_path = att.path().to_path_buf();

        let mime = build_mime_message(
            "alice@example.com",
            "brian@example.com",
            "Subject",
            "Body",
            &None,
            &[att_path],
            &[img_path],
        )
        .unwrap();
        let text = String::from_utf8(mime).unwrap();

        assert!(text.contains("Content-Type: multipart/mixed"));
        assert!(text.contains("Content-Type: multipart/related"));
        assert!(text.contains("Content-ID: <image1@corky>"));
        assert!(text.contains("Content-Disposition: inline"));
        assert!(text.contains("Content-Disposition: attachment"));
    }
}
