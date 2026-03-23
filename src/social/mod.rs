//! Social media posting module.

pub mod auth;
pub mod draft;
pub mod linkedin;
pub mod platform;
pub mod profiles;
pub mod publish;
pub mod token_store;
pub mod youtube;

use anyhow::{bail, Result};
use std::path::Path;

use crate::resolve;
use crate::util;
use draft::{DraftStatus, SocialDraft, SocialDraftMeta};
use platform::Platform;
use profiles::ProfilesFile;

/// Run the `social auth` command.
pub fn run_auth(platform_str: &str, profile: Option<&str>) -> Result<()> {
    let platform: Platform = platform_str.parse()?;
    auth::run(platform, profile)
}

/// Run the `social draft` command: create a new social draft file.
pub fn run_draft(
    platform_str: &str,
    body: Option<&str>,
    author: Option<&str>,
    visibility: &str,
    tags: &[String],
) -> Result<()> {
    let platform: Platform = platform_str.parse()?;

    let author = match author {
        Some(a) => a.to_string(),
        None => {
            // Try to get default from .corky.toml owner
            if let Some(cfg) = crate::config::corky_config::try_load_config(None) {
                if let Some(owner) = &cfg.owner {
                    if !owner.name.is_empty() {
                        owner.name.clone()
                    } else {
                        bail!("No --author given and no [owner] name in .corky.toml")
                    }
                } else {
                    bail!("No --author given and no [owner] section in .corky.toml")
                }
            } else {
                bail!("No --author given and no .corky.toml found")
            }
        }
    };

    let meta = SocialDraftMeta {
        platform,
        author,
        visibility: visibility.to_string(),
        status: DraftStatus::Draft,
        tags: tags.to_vec(),
        scheduled_at: None,
        published_at: None,
        post_id: None,
        post_url: None,
        images: vec![],
        video: None,
        captions: None,
        title: None,
    };

    let body_text = body.unwrap_or("").to_string();
    let social_draft = SocialDraft::new(meta, body_text);
    let rendered = social_draft.render()?;

    // Create file in social/ directory
    let social_dir = resolve::social_dir();
    std::fs::create_dir_all(&social_dir)?;

    let slug = generate_draft_slug(platform);
    let file_path = social_dir.join(format!("{}.md", slug));

    std::fs::write(&file_path, rendered)?;
    println!("Created social draft: {}", file_path.display());
    Ok(())
}

/// Generate a slug for a social draft file.
fn generate_draft_slug(platform: Platform) -> String {
    let now = chrono::Local::now();
    format!("{}-{}", now.format("%Y%m%d-%H%M%S"), platform)
}

/// Run the `social publish` command.
pub fn run_publish(file: &Path, dry_run: bool) -> Result<()> {
    publish::publish(file, dry_run)
}

/// Run the `youtube edit` command: update a published YouTube video's metadata.
pub fn run_youtube_edit(file: &Path) -> Result<()> {
    let content = std::fs::read_to_string(file)?;
    let draft = SocialDraft::parse(&content)?;

    if draft.meta.platform != platform::Platform::Youtube {
        bail!("Draft is not a YouTube draft (platform: {})", draft.meta.platform);
    }

    let video_id = draft.meta.post_id.clone().ok_or_else(|| {
        anyhow::anyhow!("Video has not been published yet — no post_id in frontmatter.")
    })?;

    let profiles = ProfilesFile::load()?;
    let author = &draft.meta.author;
    let urn = profiles.resolve_urn(author, platform::Platform::Youtube)?;

    let store = token_store::TokenStore::load()?;
    let token = store.get_valid(&urn).ok_or_else(|| {
        anyhow::anyhow!(
            "No valid token for {} ({}).\nRun `corky youtube auth` to authenticate.",
            author,
            urn,
        )
    })?;

    let title = draft.meta.title.clone().unwrap_or_else(|| {
        draft.body.lines().next().unwrap_or("").to_string()
    });
    let description = if draft.meta.title.is_some() {
        draft.body.clone()
    } else {
        let mut lines = draft.body.lines();
        lines.next();
        lines.collect::<Vec<_>>().join("\n").trim().to_string()
    };

    let metadata = youtube::VideoMetadata {
        title,
        description,
        tags: draft.meta.tags.clone(),
        visibility: draft.meta.visibility.clone(),
        category_id: String::new(),
    };

    youtube::update_video(&token.access_token, &video_id, &metadata)?;

    println!("Updated YouTube video: https://www.youtube.com/watch?v={}", video_id);
    Ok(())
}

/// Run the `linkedin comment` command: post a comment on a published LinkedIn post.
pub fn run_comment(file: &Path, body: &str) -> Result<()> {
    let content = std::fs::read_to_string(file)?;
    let draft = SocialDraft::parse(&content)?;

    if draft.meta.platform != platform::Platform::LinkedIn {
        bail!(
            "Draft is not a LinkedIn draft (platform: {})",
            draft.meta.platform
        );
    }

    let post_id = draft.meta.post_id.clone().ok_or_else(|| {
        anyhow::anyhow!("Post has not been published yet — no post_id in frontmatter.")
    })?;

    if body.trim().is_empty() {
        bail!("Comment text is empty.");
    }

    // Resolve author → URN → token
    let profiles = ProfilesFile::load()?;
    let author = &draft.meta.author;
    let urn = profiles.resolve_urn(author, platform::Platform::LinkedIn)?;

    let store = token_store::TokenStore::load()?;
    let token = store.get_valid(&urn).ok_or_else(|| {
        anyhow::anyhow!(
            "No valid token for {} ({}).\nRun `corky linkedin auth` to authenticate.",
            author,
            urn,
        )
    })?;

    let comment_id = linkedin::create_comment(&token.access_token, &urn, &post_id, body)?;

    let post_url = draft
        .meta
        .post_url
        .as_deref()
        .unwrap_or("(unknown URL)");
    println!("Commented on {}", post_url);
    println!("Comment ID: {}", comment_id);
    Ok(())
}

/// Run the `youtube comment` command: post a comment on a published YouTube video.
pub fn run_youtube_comment(file: &Path, body: &str) -> Result<()> {
    let content = std::fs::read_to_string(file)?;
    let draft = SocialDraft::parse(&content)?;

    if draft.meta.platform != platform::Platform::Youtube {
        bail!(
            "Draft is not a YouTube draft (platform: {})",
            draft.meta.platform
        );
    }

    let video_id = draft.meta.post_id.clone().ok_or_else(|| {
        anyhow::anyhow!("Video has not been published yet — no post_id in frontmatter.")
    })?;

    if body.trim().is_empty() {
        bail!("Comment text is empty.");
    }

    let profiles = ProfilesFile::load()?;
    let author = &draft.meta.author;
    let urn = profiles.resolve_urn(author, platform::Platform::Youtube)?;

    let store = token_store::TokenStore::load()?;
    let token = store.get_valid(&urn).ok_or_else(|| {
        anyhow::anyhow!(
            "No valid token for {} ({}).\nRun `corky youtube auth` to authenticate.",
            author,
            urn,
        )
    })?;

    let comment_id = youtube::insert_comment(&token.access_token, &video_id, body)?;

    println!("Commented on https://www.youtube.com/watch?v={}", video_id);
    println!("Comment ID: {}", comment_id);
    println!(
        "Pin it manually: https://www.youtube.com/watch?v={}&lc={}",
        video_id, comment_id
    );
    Ok(())
}

/// Get a valid YouTube access token using the first YouTube profile found.
fn get_youtube_token() -> Result<String> {
    let profiles = ProfilesFile::load()?;
    let youtube_profiles: Vec<_> = profiles
        .profiles
        .iter()
        .filter(|(_, p)| p.youtube.is_some())
        .collect();

    if youtube_profiles.is_empty() {
        bail!("No YouTube profile found in .corky.toml. Run `corky youtube auth --profile <name>` first.");
    }

    let (author, _) = youtube_profiles[0];
    let urn = profiles.resolve_urn(author, platform::Platform::Youtube)?;
    let store = token_store::TokenStore::load()?;
    let token = store.get_valid(&urn).ok_or_else(|| {
        anyhow::anyhow!(
            "No valid token for {} ({}).\nRun `corky youtube auth` to authenticate.",
            author,
            urn,
        )
    })?;
    Ok(token.access_token.clone())
}

/// Run `youtube playlist create`.
pub fn run_playlist_create(title: &str, description: &str, visibility: &str) -> Result<()> {
    let access_token = get_youtube_token()?;
    let playlist_id = youtube::create_playlist(&access_token, title, description, visibility)?;
    println!("Created playlist: https://www.youtube.com/playlist?list={}", playlist_id);
    println!("Playlist ID: {}", playlist_id);
    Ok(())
}

/// Run `youtube playlist add`.
pub fn run_playlist_add(playlist_id: &str, video_id: &str, position: Option<u32>) -> Result<()> {
    let access_token = get_youtube_token()?;
    let item_id = youtube::add_to_playlist(&access_token, playlist_id, video_id, position)?;
    println!("Added video {} to playlist {}", video_id, playlist_id);
    println!("PlaylistItem ID: {}", item_id);
    Ok(())
}

/// Run `youtube playlist list`.
pub fn run_playlist_list() -> Result<()> {
    let access_token = get_youtube_token()?;
    let playlists = youtube::list_playlists(&access_token)?;

    if playlists.is_empty() {
        println!("No playlists found.");
        return Ok(());
    }

    for (id, title, privacy, count) in &playlists {
        println!("{} | {} | {} | {} videos", id, title, privacy, count);
    }
    Ok(())
}

/// Run `youtube playlist remove`.
pub fn run_playlist_remove(playlist_id: &str, video_id: &str) -> Result<()> {
    let access_token = get_youtube_token()?;
    youtube::remove_from_playlist(&access_token, playlist_id, video_id)?;
    println!("Removed video {} from playlist {}", video_id, playlist_id);
    Ok(())
}

/// Run the `social edit` command: update a published post's commentary.
pub fn run_edit(file: &Path, body: Option<&str>) -> Result<()> {
    let content = std::fs::read_to_string(file)?;
    let draft = SocialDraft::parse(&content)?;

    let post_id = draft.meta.post_id.clone().ok_or_else(|| {
        anyhow::anyhow!("Post has not been published yet — no post_id in frontmatter.")
    })?;

    let commentary = match body {
        Some(b) => b.to_string(),
        None => draft.body.clone(),
    };

    if commentary.trim().is_empty() {
        bail!("Post body is empty. Provide --body or add text to the draft file.");
    }

    // Resolve author → URN → token (same as publish flow)
    let profiles = ProfilesFile::load()?;
    let platform = draft.meta.platform;
    let author = &draft.meta.author;
    let urn = profiles.resolve_urn(author, platform)?;

    let store = token_store::TokenStore::load()?;
    let token = store.get_valid(&urn).ok_or_else(|| {
        anyhow::anyhow!(
            "No valid token for {} ({}).\nRun `corky linkedin auth` to authenticate.",
            author,
            urn,
        )
    })?;

    linkedin::update_post(&token.access_token, &post_id, &commentary)?;

    // If body came from the file (no --body override), the file is already up to date.
    // If --body was provided, update the draft file body to match.
    if body.is_some() {
        let mut draft = draft;
        draft.body = commentary;
        let rendered = draft.render()?;
        std::fs::write(file, rendered)?;
    }

    println!("Updated LinkedIn post: https://www.linkedin.com/feed/update/{}", post_id);
    Ok(())
}

/// Run the `social check` command: validate profiles in .corky.toml (or profiles.toml fallback).
pub fn run_check() -> Result<()> {
    let profiles = match ProfilesFile::load() {
        Ok(p) => p,
        Err(_) => {
            let corky_path = resolve::corky_toml();
            println!("No [profiles] section found in {}", corky_path.display());
            println!("\nAdd profiles to .corky.toml like:");
            println!("  [profiles.btakita.linkedin]");
            println!("  handle = \"brian-takita\"");
            println!("  urn = \"urn:li:person:abc123\"");
            return Ok(());
        }
    };

    let result = profiles.validate();

    if result.errors.is_empty() && result.warnings.is_empty() && result.info.is_empty() {
        println!("profiles OK ({} profiles)", profiles.profiles.len());
        return Ok(());
    }

    for msg in &result.errors {
        eprintln!("ERROR: {}", msg);
    }
    for msg in &result.warnings {
        eprintln!("WARNING: {}", msg);
    }
    for msg in &result.info {
        println!("INFO: {}", msg);
    }

    if !result.is_ok() {
        bail!("profiles have {} error(s)", result.errors.len());
    }
    Ok(())
}

/// Run the `social list` command: list social drafts.
pub fn run_list(status_filter: Option<&str>) -> Result<()> {
    let social_dir = resolve::social_dir();
    if !social_dir.exists() {
        println!("No social drafts found.");
        return Ok(());
    }

    let filter: Option<DraftStatus> = status_filter
        .map(|s| s.parse())
        .transpose()?;

    let mut entries: Vec<_> = std::fs::read_dir(&social_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut count = 0;
    for entry in entries {
        let content = std::fs::read_to_string(entry.path())?;
        if let Ok(draft) = SocialDraft::parse(&content) {
            if let Some(ref f) = filter
                && draft.meta.status != *f {
                    continue;
                }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let preview = util::truncate_preview(&draft.body, 60);
            println!(
                "  {} [{}] {} @{} — {}",
                name_str,
                draft.meta.status,
                draft.meta.platform,
                draft.meta.author,
                preview,
            );
            count += 1;
        }
    }

    if count == 0 {
        println!("No social drafts found.");
    }
    Ok(())
}

/// Run the `social rename-author` command.
pub fn run_rename_author(old: &str, new: &str) -> Result<()> {
    let mut count = 0;

    // Rename in .corky.toml [profiles] section (or fallback profiles.toml)
    let corky_path = resolve::corky_toml();
    if corky_path.exists() {
        let content = std::fs::read_to_string(&corky_path)?;
        let mut doc = content.parse::<toml_edit::DocumentMut>()?;
        if let Some(profiles_table) = doc.get_mut("profiles").and_then(|v| v.as_table_mut())
            && let Some(item) = profiles_table.remove(old) {
                profiles_table.insert(new, item);
                std::fs::write(&corky_path, doc.to_string())?;
                println!("Renamed profile '{}' -> '{}' in .corky.toml", old, new);
                count += 1;
            }
    }
    // Also check standalone profiles.toml for backward compat
    let profiles_path = resolve::profiles_toml();
    if profiles_path.exists() {
        let content = std::fs::read_to_string(&profiles_path)?;
        let mut profiles: ProfilesFile = toml::from_str(&content)?;
        if let Some(profile) = profiles.profiles.remove(old) {
            profiles.profiles.insert(new.to_string(), profile);
            let updated = toml::to_string_pretty(&profiles)?;
            std::fs::write(&profiles_path, updated)?;
            println!("Renamed profile '{}' -> '{}' in profiles.toml", old, new);
            count += 1;
        }
    }

    // Rename in social drafts
    let social_dir = resolve::social_dir();
    if social_dir.exists() {
        for entry in std::fs::read_dir(&social_dir)? {
            let entry = entry?;
            if entry.path().extension().map(|x| x == "md").unwrap_or(false) {
                let content = std::fs::read_to_string(entry.path())?;
                if let Ok(mut draft) = SocialDraft::parse(&content)
                    && draft.meta.author == old {
                        draft.meta.author = new.to_string();
                        let rendered = draft.render()?;
                        std::fs::write(entry.path(), rendered)?;
                        println!("Updated author in {}", entry.path().display());
                        count += 1;
                    }
            }
        }
    }

    if count == 0 {
        println!("No references to '{}' found.", old);
    } else {
        println!("Renamed {} reference(s).", count);
    }
    Ok(())
}
