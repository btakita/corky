//! LinkedIn API client (REST API).

use anyhow::{bail, Result};
use serde_json::json;

/// Maximum character count for a LinkedIn post.
const MAX_BODY_LENGTH: usize = 3000;

/// Maximum images in a multi-image carousel.
const MAX_IMAGES: usize = 20;

/// Default LinkedIn API base URL.
const API_BASE: &str = "https://api.linkedin.com";

/// LinkedIn visibility values.
pub fn map_visibility(visibility: &str) -> Result<&'static str> {
    match visibility.to_lowercase().as_str() {
        "public" => Ok("PUBLIC"),
        "connections" => Ok("CONNECTIONS"),
        _ => bail!(
            "Invalid LinkedIn visibility '{}'. Valid: public, connections",
            visibility
        ),
    }
}

/// Get the authenticated user's URN via /v2/userinfo.
pub fn get_user_urn(access_token: &str) -> Result<String> {
    get_user_urn_at(API_BASE, access_token)
}

/// Get the authenticated user's URN, with configurable API base URL (for testing).
pub fn get_user_urn_at(api_base: &str, access_token: &str) -> Result<String> {
    let url = format!("{}/v2/userinfo", api_base);
    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .call()
        .map_err(|e| anyhow::anyhow!("LinkedIn userinfo request failed: {}", e))?;

    let body: serde_json::Value = resp.into_json()?;
    let sub = body["sub"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'sub' in userinfo response"))?;

    Ok(format!("urn:li:person:{}", sub))
}

/// Initialize an image upload and upload the binary data.
/// Returns the image URN for use in post creation.
pub fn upload_image(access_token: &str, author_urn: &str, image_bytes: &[u8]) -> Result<String> {
    upload_image_at(API_BASE, access_token, author_urn, image_bytes)
}

/// Upload an image with configurable API base URL (for testing).
pub fn upload_image_at(
    api_base: &str,
    access_token: &str,
    author_urn: &str,
    image_bytes: &[u8],
) -> Result<String> {
    // Step 1: Initialize upload
    let init_payload = json!({
        "initializeUploadRequest": {
            "owner": author_urn
        }
    });

    let init_url = format!("{}/rest/images?action=initializeUpload", api_base);
    let init_resp = ureq::post(&init_url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("LinkedIn-Version", "202601")
        .set("X-Restli-Protocol-Version", "2.0.0")
        .send_json(&init_payload);

    let init_body: serde_json::Value = match init_resp {
        Ok(r) => r.into_json()?,
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("LinkedIn image init failed (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("LinkedIn image init request failed: {}", e),
    };

    let upload_url = init_body["value"]["uploadUrl"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing uploadUrl in image init response"))?;
    let image_urn = init_body["value"]["image"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing image URN in image init response"))?
        .to_string();

    // Step 2: Upload binary image
    let upload_resp = ureq::put(upload_url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("Content-Type", "application/octet-stream")
        .send_bytes(image_bytes);

    match upload_resp {
        Ok(_) => {}
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("LinkedIn image upload failed (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("LinkedIn image upload request failed: {}", e),
    }

    Ok(image_urn)
}

/// Update an existing LinkedIn post's commentary via PARTIAL_UPDATE.
///
/// Returns `Ok(())` on success (HTTP 204 No Content).
pub fn update_post(access_token: &str, post_urn: &str, commentary: &str) -> Result<()> {
    update_post_at(API_BASE, access_token, post_urn, commentary)
}

/// Update a post with configurable API base URL (for testing).
pub fn update_post_at(
    api_base: &str,
    access_token: &str,
    post_urn: &str,
    commentary: &str,
) -> Result<()> {
    let char_count = commentary.chars().count();
    if char_count > MAX_BODY_LENGTH {
        bail!(
            "Post body exceeds LinkedIn's {} character limit ({} characters)",
            MAX_BODY_LENGTH,
            char_count
        );
    }

    // URL-encode the URN (colons → %3A, commas → %2C)
    let encoded_urn = post_urn
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace(',', "%2C")
        .replace('(', "%28")
        .replace(')', "%29");
    let url = format!("{}/rest/posts/{}", api_base, encoded_urn);
    let payload = serde_json::json!({
        "patch": {
            "$set": {
                "commentary": commentary
            }
        }
    });

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("X-RestLi-Method", "PARTIAL_UPDATE")
        .set("LinkedIn-Version", "202601")
        .set("Content-Type", "application/json")
        .send_json(&payload);

    match resp {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!(
                "LinkedIn API error (HTTP {}): {}",
                status,
                body
            );
        }
        Err(e) => bail!("LinkedIn API request failed: {}", e),
    }
}

/// Create a comment on a LinkedIn post.
///
/// Returns the comment URN on success.
pub fn create_comment(
    access_token: &str,
    author_urn: &str,
    post_urn: &str,
    text: &str,
) -> Result<String> {
    create_comment_at(API_BASE, access_token, author_urn, post_urn, text)
}

/// Create a comment with configurable API base URL (for testing).
pub fn create_comment_at(
    api_base: &str,
    access_token: &str,
    author_urn: &str,
    post_urn: &str,
    text: &str,
) -> Result<String> {
    let char_count = text.chars().count();
    if char_count > MAX_BODY_LENGTH {
        bail!(
            "Comment exceeds LinkedIn's {} character limit ({} characters)",
            MAX_BODY_LENGTH,
            char_count
        );
    }

    let payload = json!({
        "actor": author_urn,
        "message": {
            "text": text
        }
    });

    // URL-encode the post URN for the path
    let encoded_urn = post_urn
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace(',', "%2C")
        .replace('(', "%28")
        .replace(')', "%29");
    // Use v2 API (not /rest/) — the versioned /rest/socialActions endpoint
    // requires partner-level permissions that personal tokens don't have.
    let url = format!(
        "{}/v2/socialActions/{}/comments",
        api_base, encoded_urn
    );
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("X-Restli-Protocol-Version", "2.0.0")
        .send_json(&payload);

    match resp {
        Ok(r) => {
            let comment_id = r
                .header("x-restli-id")
                .unwrap_or("unknown")
                .to_string();
            Ok(comment_id)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("LinkedIn API error (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("LinkedIn API request failed: {}", e),
    }
}

/// Create a post on LinkedIn using the REST API.
///
/// `image_urns` controls the post type:
/// - empty: text-only post
/// - 1 image: single image post (`content.media`)
/// - 2+ images: multi-image carousel (`content.multiImage`)
pub fn create_post(
    access_token: &str,
    author_urn: &str,
    body: &str,
    visibility: &str,
    image_urns: &[String],
) -> Result<(String, String)> {
    create_post_at(API_BASE, access_token, author_urn, body, visibility, image_urns)
}

/// Create a post with configurable API base URL (for testing).
pub fn create_post_at(
    api_base: &str,
    access_token: &str,
    author_urn: &str,
    body: &str,
    visibility: &str,
    image_urns: &[String],
) -> Result<(String, String)> {
    // Validate body length
    let char_count = body.chars().count();
    if char_count > MAX_BODY_LENGTH {
        bail!(
            "Post body exceeds LinkedIn's {} character limit ({} characters)",
            MAX_BODY_LENGTH,
            char_count
        );
    }

    // Validate image count
    if image_urns.len() > MAX_IMAGES {
        bail!(
            "Too many images ({}) — LinkedIn allows up to {}",
            image_urns.len(),
            MAX_IMAGES
        );
    }

    let li_visibility = map_visibility(visibility)?;

    let mut payload = json!({
        "author": author_urn,
        "commentary": body,
        "visibility": li_visibility,
        "distribution": {
            "feedDistribution": "MAIN_FEED",
            "targetEntities": [],
            "thirdPartyDistributionChannels": []
        },
        "lifecycleState": "PUBLISHED",
        "isReshareDisabledByAuthor": false
    });

    // Add image content based on count
    match image_urns.len() {
        0 => {} // text-only, no content field needed
        1 => {
            payload["content"] = json!({
                "media": {
                    "id": image_urns[0]
                }
            });
        }
        _ => {
            let images: Vec<serde_json::Value> = image_urns
                .iter()
                .map(|urn| json!({ "id": urn }))
                .collect();
            payload["content"] = json!({
                "multiImage": {
                    "images": images
                }
            });
        }
    }

    let url = format!("{}/rest/posts", api_base);
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", access_token))
        .set("LinkedIn-Version", "202601")
        .set("X-Restli-Protocol-Version", "2.0.0")
        .send_json(&payload);

    match resp {
        Ok(r) => {
            // LinkedIn returns the post ID in the x-restli-id header
            let post_id = r
                .header("x-restli-id")
                .unwrap_or("unknown")
                .to_string();
            let post_url = format!(
                "https://www.linkedin.com/feed/update/{}",
                post_id
            );
            Ok((post_id, post_url))
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!(
                "LinkedIn API error (HTTP {}): {}",
                status,
                body
            );
        }
        Err(e) => bail!("LinkedIn API request failed: {}", e),
    }
}
