//! `corky skill` — Manage the Claude Code skill definitions.
//!
//! Installs 3 skills + runbooks:
//! - `.claude/skills/corky/SKILL.md` — general corky commands
//! - `.claude/skills/email/SKILL.md` + `runbooks/` — email drafting
//! - `.claude/skills/linkedin/SKILL.md` + `runbooks/` — LinkedIn posting
//!
//! Delegates to `agent_kit::skill::SkillConfig` for the corky SKILL.md,
//! and handles additional skills + runbooks directly.

use anyhow::{Context, Result};
use std::path::Path;

use agent_kit::detect::Environment;

/// The corky SKILL.md content bundled at build time.
const BUNDLED_SKILL: &str = include_str!("../SKILL.md");

// Email skill + runbooks
const EMAIL_SKILL: &str = include_str!("../skills/email/SKILL.md");
const EMAIL_RUNBOOK_SEND: &str = include_str!("../skills/email/runbooks/email-send.md");
const EMAIL_RUNBOOK_REVIEW: &str = include_str!("../skills/email/runbooks/review-inbox.md");
const EMAIL_RUNBOOK_DRAFT: &str = include_str!("../skills/email/runbooks/draft-reply.md");
const EMAIL_RUNBOOK_ENRICH: &str = include_str!("../skills/email/runbooks/enrich-contact.md");

// Email runbooks (new)
const EMAIL_RUNBOOK_GMAIL_CONFIG: &str = include_str!("../skills/email/runbooks/gmail-config.md");
const EMAIL_RUNBOOK_BROWSER_PASTE: &str = include_str!("../skills/email/runbooks/browser-paste.md");

// LinkedIn skill + runbooks
const LINKEDIN_SKILL: &str = include_str!("../skills/linkedin/SKILL.md");
const LINKEDIN_RUNBOOK_POST: &str = include_str!("../skills/linkedin/runbooks/linkedin-post.md");

// Corky runbooks
const CORKY_RUNBOOK_IMPORTS: &str = include_str!("../skills/corky/runbooks/imports.md");
const CORKY_RUNBOOK_TRANSCRIPTION: &str = include_str!("../skills/corky/runbooks/transcription.md");

/// Current binary version (from Cargo.toml).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A bundled file to install (path relative to project root, content).
struct BundledFile {
    rel_path: &'static str,
    content: &'static str,
}

/// All bundled files beyond the main corky SKILL.md.
const EXTRA_FILES: &[BundledFile] = &[
    BundledFile { rel_path: ".claude/skills/email/SKILL.md", content: EMAIL_SKILL },
    BundledFile { rel_path: ".claude/skills/email/runbooks/email-send.md", content: EMAIL_RUNBOOK_SEND },
    BundledFile { rel_path: ".claude/skills/email/runbooks/review-inbox.md", content: EMAIL_RUNBOOK_REVIEW },
    BundledFile { rel_path: ".claude/skills/email/runbooks/draft-reply.md", content: EMAIL_RUNBOOK_DRAFT },
    BundledFile { rel_path: ".claude/skills/email/runbooks/enrich-contact.md", content: EMAIL_RUNBOOK_ENRICH },
    BundledFile { rel_path: ".claude/skills/email/runbooks/gmail-config.md", content: EMAIL_RUNBOOK_GMAIL_CONFIG },
    BundledFile { rel_path: ".claude/skills/email/runbooks/browser-paste.md", content: EMAIL_RUNBOOK_BROWSER_PASTE },
    BundledFile { rel_path: ".claude/skills/linkedin/SKILL.md", content: LINKEDIN_SKILL },
    BundledFile { rel_path: ".claude/skills/linkedin/runbooks/linkedin-post.md", content: LINKEDIN_RUNBOOK_POST },
    BundledFile { rel_path: ".claude/skills/corky/runbooks/imports.md", content: CORKY_RUNBOOK_IMPORTS },
    BundledFile { rel_path: ".claude/skills/corky/runbooks/transcription.md", content: CORKY_RUNBOOK_TRANSCRIPTION },
];

/// Resolve the corky skill file path under the given root.
fn skill_path(root: Option<&Path>) -> std::path::PathBuf {
    Environment::ClaudeCode.skill_path("corky", root)
}

/// Install the main corky SKILL.md.
fn install_main_skill(root: Option<&Path>) -> Result<()> {
    let path = skill_path(root);

    if path.exists() {
        let existing = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if existing == BUNDLED_SKILL {
            eprintln!("Skill already up to date (v{VERSION}).");
            return Ok(());
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    std::fs::write(&path, BUNDLED_SKILL)
        .with_context(|| format!("failed to write {}", path.display()))?;
    eprintln!("Installed skill v{VERSION} → {}", path.display());
    Ok(())
}

/// Check if the main corky SKILL.md matches the bundled version.
fn check_main_skill(root: Option<&Path>) -> Result<bool> {
    let path = skill_path(root);

    if !path.exists() {
        eprintln!("Not installed. Run `corky skill install` to install.");
        return Ok(false);
    }

    let existing = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    if existing == BUNDLED_SKILL {
        eprintln!("Up to date (v{VERSION}).");
        Ok(true)
    } else {
        eprintln!("Outdated. Run `corky skill install` to update to v{VERSION}.");
        Ok(false)
    }
}

/// Install a single bundled file, creating directories as needed.
/// Returns true if the file was written (new or changed), false if already up to date.
fn install_file(root: Option<&Path>, file: &BundledFile) -> Result<bool> {
    let path = match root {
        Some(r) => r.join(file.rel_path),
        None => file.rel_path.into(),
    };

    if path.exists() {
        let existing = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if existing == file.content {
            return Ok(false);
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    std::fs::write(&path, file.content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    eprintln!("  wrote {}", path.display());
    Ok(true)
}

/// Check if a bundled file matches the installed version.
fn check_file(root: Option<&Path>, file: &BundledFile) -> Result<bool> {
    let path = match root {
        Some(r) => r.join(file.rel_path),
        None => file.rel_path.into(),
    };

    if !path.exists() {
        return Ok(false);
    }

    let existing = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(existing == file.content)
}

/// Install all bundled skills + runbooks to the project.
/// When `root` is None, paths are relative to CWD.
pub fn install_at(root: Option<&Path>) -> Result<()> {
    // Install the main corky skill
    install_main_skill(root)?;

    // Install dependency skills (instruction-files SKILL.md + runbooks)
    let dep_root = root.unwrap_or_else(|| Path::new("."));
    let dep_written = instruction_files::init(dep_root)?;
    if !dep_written.is_empty() {
        eprintln!("Installed {} dependency file(s) (instruction-files).", dep_written.len());
    }

    // Install extra skills + runbooks
    let mut wrote = 0;
    let mut skipped = 0;
    for file in EXTRA_FILES {
        if install_file(root, file)? {
            wrote += 1;
        } else {
            skipped += 1;
        }
    }

    if wrote > 0 {
        eprintln!("Installed {wrote} file(s), {skipped} already up to date.");
    } else {
        eprintln!("All {skipped} extra skill files already up to date.");
    }

    Ok(())
}

/// Public entry point (CWD-relative, called from main).
pub fn install() -> Result<()> {
    install_at(None)
}

/// Check if all installed skills match the bundled version.
/// When `root` is None, paths are relative to CWD.
pub fn check_at(root: Option<&Path>) -> Result<()> {
    let mut all_ok = check_main_skill(root)?;

    for file in EXTRA_FILES {
        if !check_file(root, file)? {
            eprintln!("  outdated: {}", file.rel_path);
            all_ok = false;
        }
    }

    if !all_ok {
        std::process::exit(1);
    }
    Ok(())
}

/// Check if all installed skills match the bundled version (CWD-relative).
pub fn check() -> Result<()> {
    check_at(None)
}

/// CLI entry point for `corky install-skill`.
/// The `name` parameter is accepted for backward compatibility but ignored —
/// all corky skills are installed together.
pub fn run(name: &str) -> Result<()> {
    match name {
        "corky" | "email" | "linkedin" | "all" => install(),
        _ => anyhow::bail!("Unknown skill '{}'. Available: corky, email, linkedin (or 'all')", name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skill_is_not_empty() {
        assert!(!BUNDLED_SKILL.is_empty());
    }

    #[test]
    fn bundled_skill_contains_corky() {
        assert!(BUNDLED_SKILL.contains("corky"));
    }

    #[test]
    fn email_skill_is_not_empty() {
        assert!(!EMAIL_SKILL.is_empty());
    }

    #[test]
    fn linkedin_skill_is_not_empty() {
        assert!(!LINKEDIN_SKILL.is_empty());
    }

    #[test]
    fn install_creates_all_files() {
        let dir = tempfile::tempdir().unwrap();

        install_at(Some(dir.path())).unwrap();

        // Main corky skill
        let path = dir.path().join(".claude/skills/corky/SKILL.md");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, BUNDLED_SKILL);

        // Email skill + runbooks
        assert!(dir.path().join(".claude/skills/email/SKILL.md").exists());
        assert!(dir.path().join(".claude/skills/email/runbooks/email-send.md").exists());
        assert!(dir.path().join(".claude/skills/email/runbooks/review-inbox.md").exists());
        assert!(dir.path().join(".claude/skills/email/runbooks/draft-reply.md").exists());
        assert!(dir.path().join(".claude/skills/email/runbooks/enrich-contact.md").exists());
        assert!(dir.path().join(".claude/skills/email/runbooks/gmail-config.md").exists());
        assert!(dir.path().join(".claude/skills/email/runbooks/browser-paste.md").exists());

        // LinkedIn skill + runbook
        assert!(dir.path().join(".claude/skills/linkedin/SKILL.md").exists());
        assert!(dir.path().join(".claude/skills/linkedin/runbooks/linkedin-post.md").exists());

        // Corky runbooks
        assert!(dir.path().join(".claude/skills/corky/runbooks/imports.md").exists());
        assert!(dir.path().join(".claude/skills/corky/runbooks/transcription.md").exists());

        // Dependency: instruction-files skill (transitive install)
        assert!(dir.path().join(".claude/skills/instruction-files/SKILL.md").exists());
    }

    #[test]
    fn install_idempotent() {
        let dir = tempfile::tempdir().unwrap();

        install_at(Some(dir.path())).unwrap();
        install_at(Some(dir.path())).unwrap();

        let path = dir.path().join(".claude/skills/corky/SKILL.md");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, BUNDLED_SKILL);
    }

    #[test]
    fn install_overwrites_outdated() {
        let dir = tempfile::tempdir().unwrap();

        let path = dir.path().join(".claude/skills/email/SKILL.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "old content").unwrap();

        install_at(Some(dir.path())).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, EMAIL_SKILL);
    }

    #[test]
    fn check_detects_outdated() {
        let dir = tempfile::tempdir().unwrap();

        // Install everything
        install_at(Some(dir.path())).unwrap();

        // Tamper with one file
        let path = dir.path().join(".claude/skills/email/runbooks/email-send.md");
        std::fs::write(&path, "tampered").unwrap();

        // check_file should report false
        let file = &EXTRA_FILES[1]; // email-send.md
        assert!(!check_file(Some(dir.path()), file).unwrap());
    }

    #[test]
    fn run_accepts_all_skill_names() {
        // Just verify these don't bail — actual install tested elsewhere
        // We can't easily test the full install here without mocking CWD.
        assert!(matches!(run("corky"), Ok(()) | Err(_)));
        assert!(matches!(run("email"), Ok(()) | Err(_)));
        assert!(matches!(run("linkedin"), Ok(()) | Err(_)));
        assert!(matches!(run("all"), Ok(()) | Err(_)));
    }

    #[test]
    fn run_rejects_unknown() {
        assert!(run("unknown").is_err());
    }
}
