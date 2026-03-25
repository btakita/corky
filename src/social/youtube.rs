//! YouTube Data API v3 client (REST API).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// YouTube API base URL.
const API_BASE: &str = "https://www.googleapis.com";

/// Upload URL base for resumable uploads.
const UPLOAD_BASE: &str = "https://www.googleapis.com/upload/youtube/v3/videos";

/// Chunk size for resumable uploads (8 MB).
const CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// Video metadata for upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMetadata {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// public, unlisted, or private
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// YouTube category ID (e.g. "22" for People & Blogs)
    #[serde(default)]
    pub category_id: String,
}

fn default_visibility() -> String {
    "public".to_string()
}

/// Map draft visibility string to YouTube privacy status.
pub fn map_visibility(visibility: &str) -> Result<&'static str> {
    match visibility.to_lowercase().as_str() {
        "public" => Ok("public"),
        "unlisted" => Ok("unlisted"),
        "private" => Ok("private"),
        _ => bail!(
            "Invalid YouTube visibility '{}'. Valid: public, unlisted, private",
            visibility
        ),
    }
}

/// Get the authenticated user's channel ID.
pub fn get_channel_id(access_token: &str) -> Result<String> {
    get_channel_id_at(API_BASE, access_token)
}

/// Get channel ID with configurable API base URL (for testing).
pub fn get_channel_id_at(api_base: &str, access_token: &str) -> Result<String> {
    let url = format!(
        "{}/youtube/v3/channels?part=id&mine=true",
        api_base
    );
    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .call()
        .map_err(|e| anyhow::anyhow!("YouTube channels request failed: {}", e))?;

    let body: serde_json::Value = resp.into_json()?;
    let items = body["items"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing 'items' in channels response"))?;

    if items.is_empty() {
        bail!("No YouTube channel found for this account");
    }

    let channel_id = items[0]["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing channel 'id' in response"))?
        .to_string();

    Ok(channel_id)
}

/// Upload a video to YouTube using the resumable upload protocol.
///
/// Streams the file from disk in chunks to avoid loading the entire video into memory.
/// Returns the video ID on success.
pub fn upload_video(
    access_token: &str,
    video_path: &Path,
    metadata: &VideoMetadata,
) -> Result<String> {
    upload_video_at(UPLOAD_BASE, access_token, video_path, metadata)
}

/// Upload a video with configurable upload URL base (for testing).
pub fn upload_video_at(
    upload_base: &str,
    access_token: &str,
    video_path: &Path,
    metadata: &VideoMetadata,
) -> Result<String> {
    if !video_path.exists() {
        bail!("Video file not found: {}", video_path.display());
    }

    let file_size = std::fs::metadata(video_path)?.len();
    let content_type = mime_guess::from_path(video_path)
        .first_or_octet_stream()
        .to_string();

    let privacy_status = map_visibility(&metadata.visibility)?;

    // Step 1: Initiate resumable upload
    let snippet = serde_json::json!({
        "snippet": {
            "title": metadata.title,
            "description": metadata.description,
            "tags": metadata.tags,
            "categoryId": if metadata.category_id.is_empty() { "22" } else { &metadata.category_id }
        },
        "status": {
            "privacyStatus": privacy_status,
            "embeddable": true
        }
    });

    let init_url = format!(
        "{}?uploadType=resumable&part=snippet,status",
        upload_base
    );

    let init_resp = ureq::post(&init_url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("Content-Type", "application/json; charset=UTF-8")
        .set("X-Upload-Content-Length", &file_size.to_string())
        .set("X-Upload-Content-Type", &content_type)
        .send_json(&snippet);

    let upload_url = match init_resp {
        Ok(r) => {
            r.header("Location")
                .ok_or_else(|| anyhow::anyhow!("Missing Location header in resumable upload init response"))?
                .to_string()
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("YouTube upload init failed (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("YouTube upload init request failed: {}", e),
    };

    // Step 2: Upload the video file in chunks
    let mut file = File::open(video_path)?;
    let mut offset: u64 = 0;
    let mut buf = vec![0u8; CHUNK_SIZE];

    loop {
        let bytes_read = file.read(&mut buf)?;
        if bytes_read == 0 {
            break;
        }

        let chunk = &buf[..bytes_read];
        let end = offset + bytes_read as u64 - 1;
        let content_range = format!("bytes {}-{}/{}", offset, end, file_size);

        let resp = ureq::put(&upload_url)
            .set("Authorization", &format!("Bearer {}", access_token))
            .set("Content-Type", &content_type)
            .set("Content-Range", &content_range)
            .send_bytes(chunk);

        match resp {
            Ok(r) => {
                // Final chunk — YouTube returns 200 or 201 with video resource
                let body_str = r.into_string()?;
                if body_str.trim().is_empty() {
                    // Some intermediate 2xx responses may have empty body — continue uploading
                    offset += bytes_read as u64;
                    continue;
                }
                let body: serde_json::Value = serde_json::from_str(&body_str)
                    .map_err(|e| anyhow::anyhow!("Failed to parse upload response JSON: {} (body: {})", e, &body_str[..body_str.len().min(200)]))?;
                let video_id = body["id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing video 'id' in upload response: {}", &body_str[..body_str.len().min(200)]))?
                    .to_string();
                return Ok(video_id);
            }
            Err(ureq::Error::Status(308, _resp)) => {
                // 308 Resume Incomplete — continue uploading
                offset += bytes_read as u64;
            }
            Err(ureq::Error::Status(status, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                bail!("YouTube upload failed (HTTP {}): {}", status, body);
            }
            Err(e) => bail!("YouTube upload request failed: {}", e),
        }
    }

    bail!("Video upload completed without receiving a video ID from YouTube")
}

/// Update a published video's metadata (title, description, tags, visibility).
///
/// Uses the YouTube Data API v3 videos.update endpoint.
pub fn update_video(
    access_token: &str,
    video_id: &str,
    metadata: &VideoMetadata,
) -> Result<()> {
    update_video_at(API_BASE, access_token, video_id, metadata)
}

/// Update a video with configurable API base URL (for testing).
pub fn update_video_at(
    api_base: &str,
    access_token: &str,
    video_id: &str,
    metadata: &VideoMetadata,
) -> Result<()> {
    let privacy_status = map_visibility(&metadata.visibility)?;

    let body = serde_json::json!({
        "id": video_id,
        "snippet": {
            "title": metadata.title,
            "description": metadata.description,
            "tags": metadata.tags,
            "categoryId": if metadata.category_id.is_empty() { "22" } else { &metadata.category_id }
        },
        "status": {
            "privacyStatus": privacy_status,
            "embeddable": true
        }
    });

    let url = format!(
        "{}/youtube/v3/videos?part=snippet,status",
        api_base
    );

    let resp = ureq::put(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("Content-Type", "application/json")
        .send_json(&body);

    match resp {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("YouTube video update failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => bail!("YouTube video update request failed: {}", e),
    }
}

/// Upload captions (subtitles) for a video.
///
/// `caption_path` should be an SRT file.
pub fn upload_captions(
    access_token: &str,
    video_id: &str,
    caption_path: &Path,
    language: &str,
    name: &str,
) -> Result<String> {
    upload_captions_at(API_BASE, access_token, video_id, caption_path, language, name)
}

/// Upload captions with configurable API base URL (for testing).
pub fn upload_captions_at(
    api_base: &str,
    access_token: &str,
    video_id: &str,
    caption_path: &Path,
    language: &str,
    name: &str,
) -> Result<String> {
    if !caption_path.exists() {
        bail!("Caption file not found: {}", caption_path.display());
    }

    let caption_bytes = std::fs::read(caption_path)?;

    let snippet = serde_json::json!({
        "snippet": {
            "videoId": video_id,
            "language": language,
            "name": name
        }
    });

    let url = format!(
        "{}/upload/youtube/v3/captions?uploadType=multipart&part=snippet",
        api_base
    );

    // Build multipart body manually (ureq v2 doesn't have built-in multipart)
    let boundary = format!("corky_boundary_{}", chrono::Utc::now().timestamp_millis());
    let metadata_json = serde_json::to_string(&snippet)?;

    let mut body = Vec::new();
    // Part 1: JSON metadata
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(metadata_json.as_bytes());
    body.extend_from_slice(b"\r\n");
    // Part 2: Caption file
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(&caption_bytes);
    body.extend_from_slice(b"\r\n");
    // Close boundary
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let content_type = format!("multipart/related; boundary={}", boundary);

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("Content-Type", &content_type)
        .send_bytes(&body);

    match resp {
        Ok(r) => {
            let resp_body: serde_json::Value = r.into_json()?;
            let caption_id = resp_body["id"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            Ok(caption_id)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("YouTube caption upload failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => bail!("YouTube caption upload request failed: {}", e),
    }
}

/// Create a new YouTube playlist.
///
/// Returns the playlist ID on success.
pub fn create_playlist(
    access_token: &str,
    title: &str,
    description: &str,
    visibility: &str,
) -> Result<String> {
    create_playlist_at(API_BASE, access_token, title, description, visibility)
}

/// Create a playlist with configurable API base URL (for testing).
pub fn create_playlist_at(
    api_base: &str,
    access_token: &str,
    title: &str,
    description: &str,
    visibility: &str,
) -> Result<String> {
    let privacy_status = map_visibility(visibility)?;

    let body = serde_json::json!({
        "snippet": {
            "title": title,
            "description": description
        },
        "status": {
            "privacyStatus": privacy_status
        }
    });

    let url = format!("{}/youtube/v3/playlists?part=snippet,status", api_base);

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("Content-Type", "application/json")
        .send_json(&body);

    match resp {
        Ok(r) => {
            let resp_body: serde_json::Value = r.into_json()?;
            let playlist_id = resp_body["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing playlist 'id' in response"))?
                .to_string();
            Ok(playlist_id)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("YouTube playlist create failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => bail!("YouTube playlist create request failed: {}", e),
    }
}

/// Add a video to a YouTube playlist.
///
/// Returns the playlistItem ID on success.
pub fn add_to_playlist(
    access_token: &str,
    playlist_id: &str,
    video_id: &str,
    position: Option<u32>,
) -> Result<String> {
    add_to_playlist_at(API_BASE, access_token, playlist_id, video_id, position)
}

/// Add to playlist with configurable API base URL (for testing).
pub fn add_to_playlist_at(
    api_base: &str,
    access_token: &str,
    playlist_id: &str,
    video_id: &str,
    position: Option<u32>,
) -> Result<String> {
    let mut snippet = serde_json::json!({
        "playlistId": playlist_id,
        "resourceId": {
            "kind": "youtube#video",
            "videoId": video_id
        }
    });

    if let Some(pos) = position {
        snippet["position"] = serde_json::json!(pos);
    }

    let body = serde_json::json!({ "snippet": snippet });
    let url = format!("{}/youtube/v3/playlistItems?part=snippet", api_base);

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("Content-Type", "application/json")
        .send_json(&body);

    match resp {
        Ok(r) => {
            let resp_body: serde_json::Value = r.into_json()?;
            let item_id = resp_body["id"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            Ok(item_id)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("YouTube playlist add failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => bail!("YouTube playlist add request failed: {}", e),
    }
}

/// List the authenticated user's YouTube playlists.
///
/// Returns a vec of (playlist_id, title, visibility, item_count).
pub fn list_playlists(
    access_token: &str,
) -> Result<Vec<(String, String, String, u64)>> {
    list_playlists_at(API_BASE, access_token)
}

/// List playlists with configurable API base URL (for testing).
pub fn list_playlists_at(
    api_base: &str,
    access_token: &str,
) -> Result<Vec<(String, String, String, u64)>> {
    let url = format!(
        "{}/youtube/v3/playlists?part=snippet,status,contentDetails&mine=true&maxResults=50",
        api_base
    );

    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .call()
        .map_err(|e| anyhow::anyhow!("YouTube playlists list failed: {}", e))?;

    let body: serde_json::Value = resp.into_json()?;
    let items = body["items"].as_array().unwrap_or(&Vec::new()).clone();

    let mut result = Vec::new();
    for item in &items {
        let id = item["id"].as_str().unwrap_or("").to_string();
        let title = item["snippet"]["title"].as_str().unwrap_or("").to_string();
        let privacy = item["status"]["privacyStatus"].as_str().unwrap_or("").to_string();
        let count = item["contentDetails"]["itemCount"].as_u64().unwrap_or(0);
        result.push((id, title, privacy, count));
    }

    Ok(result)
}

/// Remove a video from a YouTube playlist.
///
/// Finds the playlistItem by video ID and deletes it.
pub fn remove_from_playlist(
    access_token: &str,
    playlist_id: &str,
    video_id: &str,
) -> Result<()> {
    remove_from_playlist_at(API_BASE, access_token, playlist_id, video_id)
}

/// Remove from playlist with configurable API base URL (for testing).
pub fn remove_from_playlist_at(
    api_base: &str,
    access_token: &str,
    playlist_id: &str,
    video_id: &str,
) -> Result<()> {
    // First, find the playlistItem ID for this video
    let list_url = format!(
        "{}/youtube/v3/playlistItems?part=id,snippet&playlistId={}&videoId={}&maxResults=1",
        api_base, playlist_id, video_id
    );

    let list_resp = ureq::get(&list_url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .call()
        .map_err(|e| anyhow::anyhow!("YouTube playlistItems list failed: {}", e))?;

    let list_body: serde_json::Value = list_resp.into_json()?;
    let items = list_body["items"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing 'items' in playlistItems response"))?;

    if items.is_empty() {
        bail!("Video {} not found in playlist {}", video_id, playlist_id);
    }

    let item_id = items[0]["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing playlistItem 'id'"))?;

    // Delete the playlistItem
    let delete_url = format!(
        "{}/youtube/v3/playlistItems?id={}",
        api_base, item_id
    );

    let resp = ureq::delete(&delete_url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .call();

    match resp {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("YouTube playlist remove failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => bail!("YouTube playlist remove request failed: {}", e),
    }
}

/// Delete a YouTube video by its video ID.
///
/// Uses the YouTube Data API v3 videos.delete endpoint.
pub fn delete_video(
    access_token: &str,
    video_id: &str,
) -> Result<()> {
    delete_video_at(API_BASE, access_token, video_id)
}

/// Delete a video with configurable API base URL (for testing).
pub fn delete_video_at(
    api_base: &str,
    access_token: &str,
    video_id: &str,
) -> Result<()> {
    let url = format!("{}/youtube/v3/videos?id={}", api_base, video_id);

    let resp = ureq::delete(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .call();

    match resp {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("YouTube video delete failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => bail!("YouTube video delete request failed: {}", e),
    }
}

/// Insert a top-level comment on a YouTube video.
///
/// Uses the YouTube Data API v3 commentThreads.insert endpoint.
/// Returns the comment ID on success.
pub fn insert_comment(
    access_token: &str,
    video_id: &str,
    text: &str,
) -> Result<String> {
    insert_comment_at(API_BASE, access_token, video_id, text)
}

/// Insert a comment with configurable API base URL (for testing).
pub fn insert_comment_at(
    api_base: &str,
    access_token: &str,
    video_id: &str,
    text: &str,
) -> Result<String> {
    let url = format!(
        "{}/youtube/v3/commentThreads?part=snippet",
        api_base
    );

    let body = serde_json::json!({
        "snippet": {
            "videoId": video_id,
            "topLevelComment": {
                "snippet": {
                    "textOriginal": text
                }
            }
        }
    });

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("Content-Type", "application/json")
        .send_json(&body);

    match resp {
        Ok(r) => {
            let resp_body: serde_json::Value = r.into_json()?;
            let comment_id = resp_body["snippet"]["topLevelComment"]["id"]
                .as_str()
                .or_else(|| resp_body["id"].as_str())
                .unwrap_or("unknown")
                .to_string();
            Ok(comment_id)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("YouTube comment insert failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => bail!("YouTube comment insert request failed: {}", e),
    }
}
