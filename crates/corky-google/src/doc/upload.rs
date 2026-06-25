use anyhow::{Context, Result, bail};
use std::path::Path;

use crate::doc::drive::{google_workspace_mime_for_path, local_mime_for_path};
use crate::filter::gmail_auth;

const DRIVE_UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3/files";
const DRIVE_FILES_API: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVE_UPLOAD_BOUNDARY: &str = "corky_drive_upload_boundary";

#[derive(Clone, Copy)]
struct UploadOptions<'a> {
    share: bool,
    convert: bool,
    folder: Option<&'a str>,
}

struct UploadRequest {
    file_name: String,
    media_mime_type: String,
    drive_mime_type: String,
    folder_id: Option<String>,
    content_type: String,
    body: Vec<u8>,
}

struct DriveUploadEndpoints {
    upload_api: String,
    files_api: String,
}

impl Default for DriveUploadEndpoints {
    fn default() -> Self {
        Self {
            upload_api: DRIVE_UPLOAD_API.to_string(),
            files_api: DRIVE_FILES_API.to_string(),
        }
    }
}

impl DriveUploadEndpoints {
    fn upload_url(&self) -> String {
        format!(
            "{}?uploadType=multipart&fields=id,mimeType,webViewLink&supportsAllDrives=true",
            self.upload_api.trim_end_matches('/')
        )
    }

    fn permissions_url(&self, file_id: &str) -> String {
        format!(
            "{}/{}/permissions?supportsAllDrives=true",
            self.files_api.trim_end_matches('/'),
            urlencode(file_id)
        )
    }
}

/// Upload a file to Google Drive.
///
/// Uses multipart upload: metadata JSON + file content.
/// Returns the file's web view link.
pub fn run(
    file: &Path,
    share: bool,
    convert: bool,
    folder: Option<&str>,
    account: Option<&str>,
) -> Result<String> {
    let options = UploadOptions {
        share,
        convert,
        folder,
    };

    // Get OAuth token with Drive scope.
    let token = gmail_auth::get_access_token_for_user(
        Some("default"),
        gmail_auth::DRIVE_FILE_SCOPE,
        account,
    )?;

    upload_with_token(file, options, &token, &DriveUploadEndpoints::default())
}

fn upload_with_token(
    file: &Path,
    options: UploadOptions<'_>,
    token: &str,
    endpoints: &DriveUploadEndpoints,
) -> Result<String> {
    let request = build_upload_request(file, options.convert, options.folder)?;

    if request.drive_mime_type != request.media_mime_type {
        eprintln!(
            "Uploading {} ({}) as {}{}...",
            request.file_name,
            request.media_mime_type,
            request.drive_mime_type,
            folder_suffix(request.folder_id.as_deref())
        );
    } else {
        eprintln!(
            "Uploading {} ({}){}...",
            request.file_name,
            request.media_mime_type,
            folder_suffix(request.folder_id.as_deref())
        );
    }

    let resp = ureq::post(&endpoints.upload_url())
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &request.content_type)
        .send_bytes(&request.body);

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => {
            let _ = gmail_auth::clear_access_token(token);
            bail!("Drive API: unauthorized (401). Cleared the cached token; re-run `corky auth --scope drive`.");
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

    // Set sharing permissions if requested.
    if options.share {
        set_anyone_reader(token, file_id, endpoints)?;
    }

    let web_link = json["webViewLink"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://drive.google.com/file/d/{}/view", file_id));
    eprintln!("Uploaded: {}", web_link);

    if options.share {
        eprintln!("Shared: anyone with link can view");
    }

    Ok(web_link)
}

fn build_upload_request(file: &Path, convert: bool, folder: Option<&str>) -> Result<UploadRequest> {
    let file_name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("untitled")
        .to_string();

    let media_mime_type = local_mime_for_path(file);
    let drive_mime_type = if convert {
        google_workspace_mime_for_path(file)
            .map(str::to_string)
            .unwrap_or_else(|| media_mime_type.clone())
    } else {
        media_mime_type.clone()
    };
    let folder_id = folder.map(normalize_folder_id).transpose()?;

    let content =
        std::fs::read(file).with_context(|| format!("Failed to read {}", file.display()))?;

    let mut metadata = serde_json::json!({
        "name": file_name,
        "mimeType": drive_mime_type,
    });
    if let Some(folder_id) = &folder_id {
        metadata["parents"] = serde_json::json!([folder_id]);
    }

    let mut body = Vec::new();
    body.extend_from_slice(format!("--{DRIVE_UPLOAD_BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(metadata.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{DRIVE_UPLOAD_BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(format!("Content-Type: {media_mime_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(&content);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{DRIVE_UPLOAD_BOUNDARY}--\r\n").as_bytes());

    Ok(UploadRequest {
        file_name,
        media_mime_type,
        drive_mime_type,
        folder_id,
        content_type: format!("multipart/related; boundary={DRIVE_UPLOAD_BOUNDARY}"),
        body,
    })
}

fn normalize_folder_id(folder: &str) -> Result<String> {
    let folder = folder.trim();
    if folder.is_empty() {
        bail!("Drive folder cannot be empty");
    }

    for prefix in [
        "https://drive.google.com/drive/folders/",
        "http://drive.google.com/drive/folders/",
    ] {
        if let Some(rest) = folder.strip_prefix(prefix) {
            let id = id_path_segment(rest);
            if id.is_empty() {
                bail!("Drive folder URL did not include a folder ID");
            }
            return Ok(id);
        }
    }

    if let Some(id) = query_param(folder, "id") {
        return Ok(id);
    }

    Ok(folder.to_string())
}

fn id_path_segment(rest: &str) -> String {
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input.split_once('?')?.1;
    for part in query.split('&') {
        if let Some((key, value)) = part.split_once('=')
            && key == name
            && !value.is_empty()
        {
            return Some(value.to_string());
        }
    }
    None
}

fn folder_suffix(folder_id: Option<&str>) -> String {
    match folder_id {
        Some(folder_id) => format!(" to folder {folder_id}"),
        None => String::new(),
    }
}

/// Set "anyone with link can view" permission on a Drive file.
fn set_anyone_reader(token: &str, file_id: &str, endpoints: &DriveUploadEndpoints) -> Result<()> {
    let url = endpoints.permissions_url(file_id);
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

fn urlencode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use std::fs;

    #[test]
    fn upload_request_converts_markdown_and_sets_parent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.md");
        fs::write(&file, "# Notes\n").unwrap();

        let request = build_upload_request(
            &file,
            true,
            Some("https://drive.google.com/drive/folders/folder-123?usp=sharing"),
        )
        .unwrap();
        let body = String::from_utf8(request.body).unwrap();

        assert_eq!(request.file_name, "notes.md");
        assert_eq!(request.media_mime_type, "text/markdown");
        assert_eq!(
            request.drive_mime_type,
            "application/vnd.google-apps.document"
        );
        assert_eq!(request.folder_id.as_deref(), Some("folder-123"));
        assert!(body.contains("\"name\":\"notes.md\""));
        assert!(body.contains("\"mimeType\":\"application/vnd.google-apps.document\""));
        assert!(body.contains("\"parents\":[\"folder-123\"]"));
        assert!(body.contains("Content-Type: text/markdown"));
        assert!(body.contains("# Notes"));
    }

    #[test]
    fn upload_request_keeps_unsupported_convert_as_binary() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scan.pdf");
        fs::write(&file, b"%PDF").unwrap();

        let request = build_upload_request(&file, true, None).unwrap();
        let body = String::from_utf8(request.body).unwrap();

        assert_eq!(request.media_mime_type, "application/pdf");
        assert_eq!(request.drive_mime_type, "application/pdf");
        assert!(request.folder_id.is_none());
        assert!(body.contains("\"mimeType\":\"application/pdf\""));
        assert!(!body.contains("\"parents\""));
    }

    #[test]
    fn normalize_folder_id_accepts_id_folder_url_and_open_url() {
        assert_eq!(normalize_folder_id("folder-123").unwrap(), "folder-123");
        assert_eq!(
            normalize_folder_id("https://drive.google.com/drive/folders/folder-123/edit").unwrap(),
            "folder-123"
        );
        assert_eq!(
            normalize_folder_id("https://drive.google.com/open?id=folder-456").unwrap(),
            "folder-456"
        );
        assert!(normalize_folder_id("  ").is_err());
    }

    #[test]
    fn upload_with_token_posts_multipart_and_shares() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.csv");
        fs::write(&file, "name,count\nA,1\n").unwrap();

        let mut server = mockito::Server::new();
        let upload_mock = server
            .mock("POST", "/upload/drive/v3/files")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("uploadType".into(), "multipart".into()),
                Matcher::UrlEncoded("fields".into(), "id,mimeType,webViewLink".into()),
                Matcher::UrlEncoded("supportsAllDrives".into(), "true".into()),
            ]))
            .match_header("Authorization", "Bearer test-token")
            .match_header(
                "Content-Type",
                "multipart/related; boundary=corky_drive_upload_boundary",
            )
            .match_body(Matcher::Regex(
                r#"(?s).*"mimeType":"application/vnd\.google-apps\.spreadsheet".*"parents":\["folder-123"\].*name,count.*"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"file-123","mimeType":"application/vnd.google-apps.spreadsheet","webViewLink":"https://drive.example/file-123"}"#,
            )
            .create();
        let permission_mock = server
            .mock("POST", "/drive/v3/files/file-123/permissions")
            .match_query(Matcher::UrlEncoded(
                "supportsAllDrives".into(),
                "true".into(),
            ))
            .match_header("Authorization", "Bearer test-token")
            .match_body(Matcher::PartialJsonString(
                r#"{"role":"reader","type":"anyone"}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{}"#)
            .create();
        let endpoints = DriveUploadEndpoints {
            upload_api: format!("{}/upload/drive/v3/files", server.url()),
            files_api: format!("{}/drive/v3/files", server.url()),
        };

        let link = upload_with_token(
            &file,
            UploadOptions {
                share: true,
                convert: true,
                folder: Some("folder-123"),
            },
            "test-token",
            &endpoints,
        )
        .unwrap();

        upload_mock.assert();
        permission_mock.assert();
        assert_eq!(link, "https://drive.example/file-123");
    }
}
