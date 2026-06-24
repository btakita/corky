//! Publish orchestration: draft → resolve author → get token → upload images → API → update draft.

use anyhow::{Result, bail};
use chrono::Utc;
use std::path::Path;

use super::draft::{DraftStatus, SocialDraft};
use super::linkedin;
use super::platform::Platform;
use super::profiles::ProfilesFile;
use super::token_store::TokenStore;
use super::youtube;

/// Publish a social draft file. When `dry_run` is true, validates everything
/// (auth, images) but prints the payload instead of creating the post.
pub fn publish(path: &Path, dry_run: bool) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let draft = SocialDraft::parse(&content)?;

    // PB1: Check status
    // - Published → always reject (prevents double-publish)
    // - Draft + scheduled_at set → allowed (scheduling implies readiness)
    // - Draft + no scheduled_at + not dry-run → reject (manual publish requires ready)
    // - Ready → always allowed
    // - dry-run → always allowed (for testing)
    if draft.meta.status == DraftStatus::Published {
        bail!(
            "Draft has already been published.\n\
             Published at: {}",
            draft
                .meta
                .published_at
                .map(|t| t.to_string())
                .unwrap_or_default()
        );
    }
    if !dry_run && draft.meta.status != DraftStatus::Ready && draft.meta.scheduled_at.is_none() {
        bail!(
            "Draft is not ready for publishing (status: draft).\n\
             Set status to 'ready' or add scheduled_at to the frontmatter."
        );
    }

    // Resolve author in profiles.toml
    let profiles = ProfilesFile::load()?;
    let platform = draft.meta.platform;
    let author = &draft.meta.author;

    // PB3: Author not in profiles.toml
    let urn = profiles.resolve_urn(author, platform)?;

    // PB5/PB6: Token lookup
    let store = TokenStore::load()?;
    let token = store.get_valid(&urn).ok_or_else(|| {
        if store.tokens.contains_key(&urn) {
            anyhow::anyhow!(
                "Token for {} ({}) has expired.\n\
                 Run `corky linkedin auth` to re-authenticate.",
                author,
                urn,
            )
        } else {
            anyhow::anyhow!(
                "No token found for {} ({}).\n\
                 Run `corky linkedin auth --profile {}` to authenticate.",
                author,
                urn,
                author
            )
        }
    })?;

    // Upload images if present (even in dry-run, to verify they work)
    let image_urns = upload_images(path, &draft, &token.access_token, &urn, platform)?;

    if dry_run {
        println!(
            "[dry-run] Validation passed. Would publish to {}.",
            platform
        );
        println!("[dry-run] Author: {} ({})", author, urn);
        println!("[dry-run] Visibility: {}", draft.meta.visibility);
        if !image_urns.is_empty() {
            println!("[dry-run] Images uploaded: {}", image_urns.len());
            for (i, urn) in image_urns.iter().enumerate() {
                println!("[dry-run]   {}: {}", i + 1, urn);
            }
        }
        if let Some(ref video) = draft.meta.video {
            println!("[dry-run] Video: {}", video);
        }
        if let Some(ref captions) = draft.meta.captions {
            println!("[dry-run] Captions: {}", captions);
        }
        println!("[dry-run] Body ({} chars):", draft.body.len());
        println!("---");
        println!("{}", draft.body.trim());
        println!("---");
        if let Some(ref comment) = draft.meta.first_comment {
            println!("[dry-run] First comment ({} chars):", comment.len());
            println!("---");
            println!("{}", comment.trim());
            println!("---");
        }
        println!(
            "[dry-run] No post created. Set status to 'ready' and run without --dry-run to publish."
        );
        return Ok(());
    }

    // Call platform API
    let (post_id, post_url) = match platform {
        Platform::LinkedIn => linkedin::create_post(
            &token.access_token,
            &urn,
            &draft.body,
            &draft.meta.visibility,
            &image_urns,
        )?,
        Platform::Youtube => publish_youtube(path, &draft, &token.access_token)?,
        _ => bail!("Publishing not yet implemented for {}", platform),
    };

    // Update draft frontmatter before post-create reconciliation. If reconciliation
    // fails after LinkedIn created the post, this prevents a retry from duplicating it.
    let mut draft = draft;
    draft.meta.status = DraftStatus::Published;
    draft.meta.post_id = Some(post_id.clone());
    draft.meta.post_url = Some(post_url.clone());
    draft.meta.published_at = Some(Utc::now());

    let rendered = draft.render()?;
    std::fs::write(path, rendered)?;

    if platform == Platform::LinkedIn
        && let Err(err) = linkedin::update_post(&token.access_token, &post_id, &draft.body)
    {
        bail!(
            "LinkedIn post was created at {}, but post-create body reconciliation failed: {}.\n\
             The draft was marked published to prevent a duplicate post; rerun `corky linkedin edit {}` to repair it.",
            post_url,
            err,
            path.display()
        );
    }

    // Post first comment if declared in frontmatter
    if platform == Platform::LinkedIn {
        if let Some(ref comment) = draft.meta.first_comment {
            match linkedin::create_comment(&token.access_token, &urn, &post_id, comment) {
                Ok(comment_id) => {
                    println!("Posted first comment: {}", comment_id);
                }
                Err(err) => {
                    eprintln!(
                        "Warning: first comment failed (post is live at {}): {}",
                        post_url, err
                    );
                }
            }
        }
    }

    println!("Published to {}: {}", platform, post_url);
    Ok(())
}

/// Resolve image paths relative to the draft file and upload them.
/// Returns a list of image URNs for the platform API.
fn upload_images(
    draft_path: &Path,
    draft: &SocialDraft,
    access_token: &str,
    author_urn: &str,
    platform: Platform,
) -> Result<Vec<String>> {
    if draft.meta.images.is_empty() {
        return Ok(vec![]);
    }

    let draft_dir = draft_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine parent directory of draft file"))?;

    let mut urns = Vec::new();
    for image_path_str in &draft.meta.images {
        let image_path = draft_dir.join(image_path_str);
        if !image_path.exists() {
            bail!(
                "Image file not found: {} (resolved from draft directory: {})",
                image_path.display(),
                draft_dir.display()
            );
        }

        let image_bytes = std::fs::read(&image_path)?;

        let urn = match platform {
            Platform::LinkedIn => linkedin::upload_image(access_token, author_urn, &image_bytes)?,
            _ => bail!("Image upload not yet implemented for {}", platform),
        };

        urns.push(urn);
    }

    Ok(urns)
}

/// Publish a YouTube video draft.
///
/// Reads the video file path from the draft's `video` field, uploads
/// the video, optionally uploads captions, and returns (video_id, url).
fn publish_youtube(
    draft_path: &Path,
    draft: &SocialDraft,
    access_token: &str,
) -> Result<(String, String)> {
    let video_path_str = draft.meta.video.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "YouTube draft is missing the 'video' field in frontmatter.\n\
             Add `video: path/to/video.mp4` to the YAML frontmatter."
        )
    })?;

    let draft_dir = draft_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine parent directory of draft file"))?;

    let video_path = draft_dir.join(video_path_str);
    if !video_path.exists() {
        bail!(
            "Video file not found: {} (resolved from draft directory: {})",
            video_path.display(),
            draft_dir.display()
        );
    }

    // Derive title: frontmatter title > first line of body > filename
    let title = if let Some(ref t) = draft.meta.title {
        t.clone()
    } else {
        let first_line = draft.body.lines().next().unwrap_or("").trim();
        if first_line.is_empty() {
            video_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Untitled".to_string())
        } else {
            first_line.to_string()
        }
    };

    // Description: body after the first line (if title came from body), or full body
    let description = if draft.meta.title.is_some() {
        draft.body.trim().to_string()
    } else {
        let mut lines = draft.body.lines();
        lines.next(); // skip title line
        lines.collect::<Vec<_>>().join("\n").trim().to_string()
    };

    let metadata = youtube::VideoMetadata {
        title,
        description,
        tags: draft.meta.tags.clone(),
        visibility: draft.meta.visibility.clone(),
        category_id: String::new(),
    };

    println!("Uploading video: {}", video_path.display());
    let video_id = youtube::upload_video(access_token, &video_path, &metadata)?;

    // Upload captions if provided
    if let Some(ref captions_str) = draft.meta.captions {
        let captions_path = draft_dir.join(captions_str);
        if !captions_path.exists() {
            bail!(
                "Caption file not found: {} (resolved from draft directory: {})",
                captions_path.display(),
                draft_dir.display()
            );
        }
        println!("Uploading captions: {}", captions_path.display());
        youtube::upload_captions(access_token, &video_id, &captions_path, "en", "English")?;
    }

    let post_url = format!("https://www.youtube.com/watch?v={}", video_id);
    Ok((video_id, post_url))
}
