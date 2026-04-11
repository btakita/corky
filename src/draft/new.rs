//! Scaffold a new draft markdown file.

use anyhow::Result;
use chrono::Local;
use std::path::PathBuf;

use crate::config::corky_config;
use crate::resolve;
use crate::util;

/// Create a new draft file with the given metadata fields.
#[allow(clippy::too_many_arguments)]
pub fn run(
    subject: &str,
    to: &str,
    cc: Option<&str>,
    account: Option<&str>,
    from: Option<&str>,
    in_reply_to: Option<&str>,
    thread_id: Option<&str>,
    mailbox: Option<&str>,
    attachments: &[String],
    images: &[String],
) -> Result<()> {
    let drafts_dir = match mailbox {
        Some(name) => resolve::mailbox_dir(name).join("drafts"),
        None => resolve::drafts_dir(),
    };
    std::fs::create_dir_all(&drafts_dir)?;

    // Resolve author name from [owner] in .corky.toml
    let author = corky_config::try_load_config(None)
        .and_then(|cfg| cfg.owner)
        .map(|o| o.name)
        .unwrap_or_default();

    let date = Local::now().format("%Y-%m-%d").to_string();
    let slug = util::slugify(subject);
    let path = unique_path(&drafts_dir, &date, &slug);

    let content = render(subject, to, cc, account, from, in_reply_to, thread_id, &author, attachments, images);
    std::fs::write(&path, content)?;
    println!("{}", path.display());
    Ok(())
}

/// Find a unique filename, appending -2, -3, etc. on collision.
fn unique_path(dir: &std::path::Path, date: &str, slug: &str) -> PathBuf {
    let base = dir.join(format!("{}-{}.md", date, slug));
    if !base.exists() {
        return base;
    }
    let mut n = 2u32;
    loop {
        let candidate = dir.join(format!("{}-{}-{}.md", date, slug, n));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Render the draft markdown content in YAML frontmatter format.
#[allow(clippy::too_many_arguments)]
fn render(
    subject: &str,
    to: &str,
    cc: Option<&str>,
    account: Option<&str>,
    from: Option<&str>,
    in_reply_to: Option<&str>,
    thread_id: Option<&str>,
    author: &str,
    attachments: &[String],
    images: &[String],
) -> String {
    let mut fm_lines = Vec::new();
    fm_lines.push(format!("to: {}", to));
    if let Some(cc) = cc {
        fm_lines.push(format!("cc: {}", cc));
    }
    fm_lines.push("status: draft".to_string());
    if !author.is_empty() {
        fm_lines.push(format!("author: {}", author));
    }
    if let Some(account) = account {
        fm_lines.push(format!("account: {}", account));
    }
    if let Some(from) = from {
        fm_lines.push(format!("from: {}", from));
    }
    if let Some(in_reply_to) = in_reply_to {
        fm_lines.push(format!("in_reply_to: \"{}\"", in_reply_to));
    }
    if let Some(thread_id) = thread_id {
        fm_lines.push(format!("thread_id: \"{}\"", thread_id));
    }
    if !attachments.is_empty() {
        fm_lines.push("attachments:".to_string());
        for path in attachments {
            fm_lines.push(format!("  - {}", path));
        }
    }
    if !images.is_empty() {
        fm_lines.push("images:".to_string());
        for path in images {
            fm_lines.push(format!("  - {}", path));
        }
    }

    let mut lines = Vec::new();
    lines.push("---".to_string());
    lines.extend(fm_lines);
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(format!("# {}", subject));
    lines.push(String::new());
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_minimal() {
        let out = render("Hello", "a@b.com", None, None, None, None, None, "", &[], &[]);
        assert!(out.starts_with("---\n"));
        assert!(out.contains("to: a@b.com\n"));
        assert!(out.contains("status: draft\n"));
        assert!(!out.contains("author:"));
        assert!(!out.contains("cc:"));
        assert!(out.contains("# Hello\n"));
        // Subject is after the frontmatter
        let parts: Vec<&str> = out.splitn(3, "---").collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[2].contains("# Hello"));
    }

    #[test]
    fn test_render_all_fields() {
        let out = render(
            "Test",
            "a@b.com",
            Some("c@d.com"),
            Some("personal"),
            Some("me@x.com"),
            Some("<msg-1>"),
            Some("abc123threadid"),
            "Alice",
            &[],
            &[],
        );
        assert!(out.contains("cc: c@d.com\n"));
        assert!(out.contains("author: Alice\n"));
        assert!(out.contains("account: personal\n"));
        assert!(out.contains("from: me@x.com\n"));
        assert!(out.contains("in_reply_to: \"<msg-1>\"\n"));
        assert!(out.contains("thread_id: \"abc123threadid\"\n"));
        assert!(out.contains("# Test\n"));
    }

    #[test]
    fn test_render_produces_parseable_yaml() {
        let out = render(
            "Test Subject",
            "a@b.com",
            Some("c@d.com"),
            Some("personal"),
            Some("me@x.com"),
            None,
            None,
            "Alice",
            &[],
            &[],
        );
        // Should be parseable by the YAML parser
        assert!(out.starts_with("---\n"));
        let after_first = &out[4..];
        let end = after_first.find("\n---").unwrap();
        let yaml_str = &after_first[..end];
        let meta: crate::draft::EmailDraftMeta = serde_yaml::from_str(yaml_str).unwrap();
        assert_eq!(meta.to, "a@b.com");
        assert_eq!(meta.cc.as_deref(), Some("c@d.com"));
        assert_eq!(meta.status, "draft");
        assert_eq!(meta.author.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_render_with_attachments() {
        let attachments = vec![
            "/tmp/screenshot.png".to_string(),
            "/tmp/doc.pdf".to_string(),
        ];
        let out = render("Test", "a@b.com", None, None, None, None, None, "", &attachments, &[]);
        assert!(out.contains("attachments:\n"));
        assert!(out.contains("  - /tmp/screenshot.png\n"));
        assert!(out.contains("  - /tmp/doc.pdf\n"));

        // Should be parseable
        let after_first = &out[4..];
        let end = after_first.find("\n---").unwrap();
        let yaml_str = &after_first[..end];
        let meta: crate::draft::EmailDraftMeta = serde_yaml::from_str(yaml_str).unwrap();
        assert_eq!(meta.attachments.len(), 2);
        assert_eq!(meta.attachments[0], "/tmp/screenshot.png");
        assert_eq!(meta.attachments[1], "/tmp/doc.pdf");
    }

    #[test]
    fn test_render_with_images() {
        let images = vec![
            "screenshot.png".to_string(),
            "photo.jpg".to_string(),
        ];
        let out = render("Test", "a@b.com", None, None, None, None, None, "", &[], &images);
        assert!(out.contains("images:\n"));
        assert!(out.contains("  - screenshot.png\n"));
        assert!(out.contains("  - photo.jpg\n"));

        // Should be parseable
        let after_first = &out[4..];
        let end = after_first.find("\n---").unwrap();
        let yaml_str = &after_first[..end];
        let meta: crate::draft::EmailDraftMeta = serde_yaml::from_str(yaml_str).unwrap();
        assert_eq!(meta.images.len(), 2);
        assert_eq!(meta.images[0], "screenshot.png");
        assert_eq!(meta.images[1], "photo.jpg");
    }

    #[test]
    fn test_unique_path_no_collision() {
        let dir = std::env::temp_dir().join("corky-test-unique-no-collision");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = unique_path(&dir, "2026-02-22", "hello");
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "2026-02-22-hello.md");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_unique_path_collision() {
        let dir = std::env::temp_dir().join("corky-test-unique-collision");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-02-22-hello.md"), "x").unwrap();
        let p = unique_path(&dir, "2026-02-22", "hello");
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "2026-02-22-hello-2.md");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
