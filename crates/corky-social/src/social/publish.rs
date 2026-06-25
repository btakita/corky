//! Publish orchestration: draft → resolve author → get token → upload images → API → update draft.

use anyhow::{Result, bail};
use chrono::Utc;
use std::path::Path;

use super::draft::{DraftStatus, PublishEvent, SocialDraft};
use super::linkedin;
use super::platform::Platform;
use super::profiles::ProfilesFile;
use super::token_store::TokenStore;
use super::youtube;

/// Publish a social draft file. When `dry_run` is true, validates everything
/// (auth, images) but prints the payload instead of creating the post.
pub fn publish(path: &Path, dry_run: bool) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut draft = SocialDraft::parse(&content)?;

    // PB1 (centralized in the DraftStatus state machine): `ready` folds in the
    // legacy "status is Ready, or scheduled_at is set, or this is a dry-run" rule.
    // `Publishing` is accepted here so an interrupted attempt can be reconciled below.
    let ready = dry_run || draft.meta.scheduled_at.is_some();
    draft.meta.status.can_publish(ready)?;

    // Crash recovery: a draft left in `Publishing` means a prior attempt was
    // interrupted. If it recorded a post_id the post already exists, so we must
    // reconcile (finalize without re-creating) rather than double-post. Without a
    // post_id we cannot know whether the platform created the post, so refuse to
    // retry automatically and ask the operator to verify.
    if !dry_run && draft.meta.status == DraftStatus::Publishing && draft.meta.post_id.is_none() {
        bail!(
            "A previous publish attempt for this draft was interrupted before a post id \
             was recorded.\n\
             The post may or may not exist on {}. Verify on the platform, then either set \
             status to 'published' (with the post_id) or 'ready' to retry.",
            draft.meta.platform
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

    // Reconcile an interrupted attempt (status `publishing` + recorded post_id):
    // the post already exists, so skip the create call and finish the post-create
    // steps instead of double-posting.
    let resuming = draft.meta.status == DraftStatus::Publishing && draft.meta.post_id.is_some();

    let (post_id, post_url) = if resuming {
        let post_id = draft.meta.post_id.clone().expect("checked by `resuming`");
        let post_url = draft.meta.post_url.clone().unwrap_or_default();
        println!(
            "Resuming interrupted publish (post already created): {}",
            post_url
        );
        (post_id, post_url)
    } else {
        // Persist the crash-safe `Publishing` marker BEFORE the platform API call so
        // a crash mid-create is detectable (and refuses an automatic re-post).
        draft.meta.status = draft.meta.status.transition(PublishEvent::Start)?;
        draft.meta.publish_started_at = Some(Utc::now());
        std::fs::write(path, draft.render()?)?;

        let created = match platform {
            Platform::LinkedIn => linkedin::create_post(
                &token.access_token,
                &urn,
                &draft.body,
                &draft.meta.visibility,
                &image_urns,
            ),
            Platform::Youtube => publish_youtube(path, &draft, &token.access_token),
            _ => bail!("Publishing not yet implemented for {}", platform),
        };

        match created {
            Ok((post_id, post_url)) => {
                // Checkpoint the post_id while STILL `publishing`, so a crash before
                // the terminal write reconciles (by post_id) instead of re-posting.
                draft.meta.post_id = Some(post_id.clone());
                draft.meta.post_url = Some(post_url.clone());
                std::fs::write(path, draft.render()?)?;
                (post_id, post_url)
            }
            Err(err) => {
                // Mark Failed so the scheduler stops auto-retrying; the operator
                // resets status to 'ready' to try again.
                if let Ok(failed) = draft.meta.status.transition(PublishEvent::Failed) {
                    draft.meta.status = failed;
                    if let Ok(rendered) = draft.render() {
                        let _ = std::fs::write(path, rendered);
                    }
                }
                return Err(err);
            }
        }
    };

    // Best-effort post-create body reconciliation. On failure the draft stays
    // `publishing` with its post_id, so a retry reconciles instead of duplicating.
    if platform == Platform::LinkedIn
        && let Err(err) = linkedin::update_post(&token.access_token, &post_id, &draft.body)
    {
        bail!(
            "LinkedIn post was created at {}, but post-create body reconciliation failed: {}.\n\
             The draft is marked `publishing` with its post_id, so rerunning `corky linkedin publish {}` reconciles instead of duplicating.",
            post_url,
            err,
            path.display()
        );
    }

    // Post first comment if declared in frontmatter (warn-only; never blocks finalize).
    if platform == Platform::LinkedIn
        && let Some(ref comment) = draft.meta.first_comment
    {
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

    // Terminal write: `Publishing` → `Published`.
    draft.meta.status = draft.meta.status.transition(PublishEvent::Succeeded)?;
    draft.meta.published_at = Some(Utc::now());
    std::fs::write(path, draft.render()?)?;

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
