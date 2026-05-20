use anyhow::{Result, bail};
use std::path::Path;

use crate::doc::drive::{google_workspace_mime_for_path, local_mime_for_path};
use crate::filter::gmail_auth;

const DRIVE_UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&fields=id,mimeType,webViewLink";

/// Upload a file to Google Drive.
///
/// Uses multipart upload: metadata JSON + file content.
/// Returns the file's web view link.
pub fn run(file: &Path, share: bool, convert: bool, account: Option<&str>) -> Result<String> {
    let file_name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("untitled");

    let media_mime_type = local_mime_for_path(file);
    let drive_mime_type = if convert {
        google_workspace_mime_for_path(file)
            .map(str::to_string)
            .unwrap_or_else(|| media_mime_type.clone())
    } else {
        media_mime_type.clone()
    };

    // Get OAuth token with Drive scope
    let token = gmail_auth::get_access_token_for_user(
        Some("default"),
        gmail_auth::DRIVE_FILE_SCOPE,
        account,
    )?;

    // Read file content
    let content = std::fs::read(file)?;

    // Build multipart body
    let boundary = "corky_drive_upload_boundary";
    let metadata = serde_json::json!({
        "name": file_name,
        "mimeType": drive_mime_type,
    });

    let mut body = Vec::new();
    // Metadata part
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(metadata.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");
    // File content part
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(format!("Content-Type: {media_mime_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(&content);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let content_type = format!("multipart/related; boundary={boundary}");

    // Upload
    if convert && drive_mime_type != media_mime_type {
        eprintln!(
            "Uploading {} ({}) as {}...",
            file_name, media_mime_type, drive_mime_type
        );
    } else {
        eprintln!("Uploading {} ({})...", file_name, media_mime_type);
    }
    let resp = ureq::post(DRIVE_UPLOAD_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &content_type)
        .send_bytes(&body);

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => {
            bail!("Drive API: unauthorized (401). Re-run `corky doc upload` to re-authenticate.");
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Drive API error (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("Drive API request failed: {}", e),
    };

    let json: serde_json::Value = resp.into_json()?;
    let file_id = json["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No file ID in Drive response"))?;

    // Set sharing permissions if requested
    if share {
        set_anyone_reader(&token, file_id)?;
    }

    let web_link = json["webViewLink"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://drive.google.com/file/d/{}/view", file_id));
    eprintln!("Uploaded: {}", web_link);

    if share {
        eprintln!("Shared: anyone with link can view");
    }

    Ok(web_link)
}

/// Set "anyone with link can view" permission on a Drive file.
fn set_anyone_reader(token: &str, file_id: &str) -> Result<()> {
    let url = format!(
        "https://www.googleapis.com/drive/v3/files/{}/permissions",
        file_id
    );
    let body = serde_json::json!({
        "role": "reader",
        "type": "anyone",
    });

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string());

    match resp {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Drive permissions error (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("Drive permissions request failed: {}", e),
    }
}
