//! Attach files or clipboard content to an existing draft.

use anyhow::{bail, Result};
use std::path::Path;

use super::{is_yaml_format, parse_draft_yaml, EmailDraftMeta};

/// Detect the available clipboard tool for reading image data.
fn detect_clipboard_tool() -> Option<(&'static str, &'static [&'static str])> {
    // Wayland
    if std::env::var("WAYLAND_DISPLAY").is_ok()
        && which("wl-paste") {
            return Some(("wl-paste", &["--type", "image/png"]));
        }
    // X11
    if std::env::var("DISPLAY").is_ok()
        && which("xclip") {
            return Some(("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"]));
        }
    // macOS
    if which("pbpaste") {
        // pngpaste is the standard tool for image clipboard on macOS
        if which("pngpaste") {
            return Some(("pngpaste", &["-"]));
        }
    }
    None
}

/// Check whether an executable is on PATH.
fn which(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Try to read an image from the clipboard, saving to `output_path`.
/// Returns true if an image was captured.
fn grab_clipboard_image(output_path: &Path) -> Result<bool> {
    let Some((tool, args)) = detect_clipboard_tool() else {
        bail!(
            "No clipboard tool found.\n\
             Install one of: wl-paste (Wayland), xclip (X11), pngpaste (macOS)"
        );
    };

    // For pngpaste, the output path is passed as the argument instead of "-"
    let result = if tool == "pngpaste" {
        std::process::Command::new(tool)
            .arg(output_path.to_string_lossy().as_ref())
            .output()?
    } else {
        std::process::Command::new(tool)
            .args(args)
            .output()?
    };

    if !result.status.success() {
        return Ok(false);
    }

    // For tools that write to stdout, save the output
    if tool != "pngpaste" && !result.stdout.is_empty() {
        std::fs::write(output_path, &result.stdout)?;
        return Ok(true);
    }

    // For pngpaste, check if the file was created
    if tool == "pngpaste" {
        return Ok(output_path.exists());
    }

    Ok(false)
}

/// Try to read text from the clipboard.
fn grab_clipboard_text() -> Result<Option<String>> {
    // Try wl-paste first (Wayland), then xclip, then pbpaste
    let tools: &[(&str, &[&str])] = &[
        ("wl-paste", &["--type", "text/plain"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("pbpaste", &[]),
    ];

    for (tool, args) in tools {
        if !which(tool) {
            continue;
        }
        let output = std::process::Command::new(tool)
            .args(*args)
            .output()?;
        if output.status.success() && !output.stdout.is_empty() {
            return Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()));
        }
    }
    Ok(None)
}

/// Update a draft file's YAML frontmatter, adding paths to `attachments` or `images`.
fn add_to_draft(draft_path: &Path, paths: &[String], inline: bool) -> Result<()> {
    let text = std::fs::read_to_string(draft_path)?;
    if !is_yaml_format(&text) {
        bail!("Draft is not in YAML format: {}", draft_path.display());
    }

    let after_first = &text[4..]; // skip "---\n"
    let end = after_first.find("\n---").ok_or_else(|| {
        anyhow::anyhow!("Missing closing YAML frontmatter delimiter")
    })?;
    let yaml_str = &after_first[..end];
    let rest = &after_first[end..]; // includes "\n---" and body

    let mut meta: EmailDraftMeta = serde_yaml::from_str(yaml_str)?;

    if inline {
        for p in paths {
            if !meta.images.contains(p) {
                meta.images.push(p.clone());
            }
        }
    } else {
        for p in paths {
            if !meta.attachments.contains(p) {
                meta.attachments.push(p.clone());
            }
        }
    }

    let new_yaml = serde_yaml::to_string(&meta)?;
    let updated = format!("---\n{}{}", new_yaml, rest);
    std::fs::write(draft_path, updated)?;
    Ok(())
}

/// `corky draft attach FILE [--file PATH...] [--clipboard] [--inline]`
pub fn run(draft_path: &Path, files: &[String], clipboard: bool, inline: bool) -> Result<()> {
    if !draft_path.exists() {
        bail!("Draft not found: {}", draft_path.display());
    }

    let text = std::fs::read_to_string(draft_path)?;
    if !is_yaml_format(&text) {
        bail!("Draft must use YAML frontmatter format: {}", draft_path.display());
    }

    let mut to_add: Vec<String> = Vec::new();

    // Add explicit file paths
    for f in files {
        let path = Path::new(f);
        if !path.exists() {
            bail!("File not found: {}", f);
        }
        to_add.push(f.clone());
    }

    // Handle clipboard
    if clipboard {
        let draft_dir = draft_path.parent().unwrap_or(Path::new("."));

        // Try image first
        let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let image_filename = format!("clipboard-{}.png", timestamp);
        let image_path = draft_dir.join(&image_filename);

        if let Ok(true) = grab_clipboard_image(&image_path) {
            println!("Saved clipboard image: {}", image_path.display());
            to_add.push(image_filename);
        } else {
            // Fall back to text clipboard
            if let Some(text) = grab_clipboard_text()? {
                if inline {
                    bail!("Clipboard contains text, not an image. Remove --inline to append text to the draft body.");
                }
                // Append text to the draft body
                let mut content = std::fs::read_to_string(draft_path)?;
                if !content.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str(&text);
                content.push('\n');
                std::fs::write(draft_path, content)?;
                println!("Appended clipboard text ({} chars) to draft body.", text.len());
                return Ok(());
            } else {
                bail!("Clipboard is empty (no image or text found).");
            }
        }
    }

    if to_add.is_empty() {
        bail!("No files to attach. Use --file PATH or --clipboard.");
    }

    // Validate that files exist before modifying the draft
    let draft_dir = draft_path.parent().unwrap_or(Path::new("."));
    for f in &to_add {
        let p = Path::new(f);
        let resolved = if p.is_absolute() { p.to_path_buf() } else { draft_dir.join(p) };
        if !resolved.exists() {
            bail!("File not found: {} (resolved: {})", f, resolved.display());
        }
    }

    let field = if inline { "images" } else { "attachments" };
    add_to_draft(draft_path, &to_add, inline)?;

    for f in &to_add {
        println!("Added to {}: {}", field, f);
    }

    // Show current state
    let updated_text = std::fs::read_to_string(draft_path)?;
    if let Some(meta) = parse_draft_yaml(&updated_text) {
        if !meta.attachments.is_empty() {
            println!("Attachments: {:?}", meta.attachments);
        }
        if !meta.images.is_empty() {
            println!("Images: {:?}", meta.images);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_add_attachment_to_draft() {
        let content = "---\nto: alice@example.com\nstatus: draft\n---\n\n# Test\n\nBody\n";
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        // Create a fake attachment file in the same dir
        let dir = tmp.path().parent().unwrap();
        let attach_path = dir.join("test.pdf");
        std::fs::write(&attach_path, "fake pdf").unwrap();

        add_to_draft(tmp.path(), &["test.pdf".to_string()], false).unwrap();

        let updated = std::fs::read_to_string(tmp.path()).unwrap();
        let meta = parse_draft_yaml(&updated).unwrap();
        assert_eq!(meta.attachments, vec!["test.pdf"]);
        assert!(meta.images.is_empty());

        std::fs::remove_file(&attach_path).unwrap();
    }

    #[test]
    fn test_add_inline_image_to_draft() {
        let content = "---\nto: alice@example.com\nstatus: draft\n---\n\n# Test\n\nBody\n";
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        add_to_draft(tmp.path(), &["screenshot.png".to_string()], true).unwrap();

        let updated = std::fs::read_to_string(tmp.path()).unwrap();
        let meta = parse_draft_yaml(&updated).unwrap();
        assert!(meta.attachments.is_empty());
        assert_eq!(meta.images, vec!["screenshot.png"]);
    }

    #[test]
    fn test_no_duplicate_entries() {
        let content = "---\nto: alice@example.com\nstatus: draft\nattachments:\n  - test.pdf\n---\n\n# Test\n";
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        add_to_draft(tmp.path(), &["test.pdf".to_string()], false).unwrap();

        let updated = std::fs::read_to_string(tmp.path()).unwrap();
        let meta = parse_draft_yaml(&updated).unwrap();
        assert_eq!(meta.attachments.len(), 1);
    }

    #[test]
    fn test_which_detects_ls() {
        // `ls` should be on every system
        assert!(which("ls"));
        assert!(!which("nonexistent_tool_xyz_123"));
    }

    #[test]
    fn test_rejects_legacy_format() {
        let content = "# Subject\n\n**To**: alice@example.com\n\n---\n\nBody\n";
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();

        let err = add_to_draft(tmp.path(), &["file.pdf".to_string()], false);
        assert!(err.is_err());
    }
}
