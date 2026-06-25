//! Regenerate template files in shared mailbox repos.

use anyhow::Result;
use std::path::Path;
use std::process::Command;

use crate::accounts::load_owner;
use crate::config::corky_config;
use crate::resolve;

use super::templates::{generate_agents_md, generate_readme_md};

fn run_git(args: &[&str]) -> (String, String, i32) {
    // #ckyconfigunwrap: a missing `git` binary must not panic the process.
    // Return code -1 with the spawn error in stderr; callers already branch on
    // the exit code and surface stderr.
    match Command::new(args[0]).args(&args[1..]).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let code = output.status.code().unwrap_or(-1);
            (stdout, stderr, code)
        }
        Err(e) => (
            String::new(),
            format!("failed to run `{}`: {e}", args.join(" ")),
            -1,
        ),
    }
}

/// Regenerate template files for one mailbox.
fn regenerate(display_name: &str, owner_name: &str, mb_path: &Path) -> Result<()> {
    // AGENTS.md
    std::fs::write(
        mb_path.join("AGENTS.md"),
        generate_agents_md(display_name, owner_name),
    )?;
    println!("  Updated AGENTS.md");

    // CLAUDE.md symlink
    let claude_md = mb_path.join("CLAUDE.md");
    if claude_md.exists() || claude_md.is_symlink() {
        std::fs::remove_file(&claude_md)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("AGENTS.md", &claude_md)?;
    println!("  Updated CLAUDE.md -> AGENTS.md");

    // README.md
    std::fs::write(
        mb_path.join("README.md"),
        generate_readme_md(display_name, owner_name),
    )?;
    println!("  Updated README.md");

    // .gitignore
    std::fs::write(
        mb_path.join(".gitignore"),
        "AGENTS.local.md\nCLAUDE.local.md\n__pycache__/\n",
    )?;
    println!("  Updated .gitignore");

    // voice.md
    let voice_file = resolve::voice_md();
    if voice_file.exists() {
        std::fs::copy(&voice_file, mb_path.join("voice.md"))?;
        println!("  Updated voice.md");
    }

    // .claude/skills/corky/
    crate::skill::install_at(Some(mb_path))?;
    println!("  Updated .claude/skills/corky/SKILL.md");

    Ok(())
}

/// Pull, regenerate templates, commit, and push for one mailbox.
fn reset_one(name: &str, owner_name: &str, do_sync: bool) -> Result<()> {
    let mb_path = resolve::mailbox_dir(name);
    if !mb_path.exists() {
        println!("  {}: not found at {} -- skipping", name, mb_path.display());
        return Ok(());
    }

    let is_git = mb_path.join(".git").exists();

    println!("Resetting {}...", name);
    let sp = mb_path.to_string_lossy().to_string();

    // 1. Pull latest (only for git repos)
    if do_sync && is_git {
        let (stdout, _, code) = run_git(&["git", "-C", &sp, "pull", "--rebase"]);
        if code == 0 {
            if !stdout.contains("Already up to date") {
                println!("  Pulled changes");
            }
        } else {
            println!("  Pull failed -- continuing with reset");
        }
    }

    // 2. Regenerate template files
    regenerate(name, owner_name, &mb_path)?;

    if !do_sync || !is_git {
        return Ok(());
    }

    // 3. Stage, commit, push
    run_git(&["git", "-C", &sp, "add", "-A"]);

    let (status_out, _, _) = run_git(&["git", "-C", &sp, "status", "--porcelain"]);
    if !status_out.trim().is_empty() {
        run_git(&[
            "git",
            "-C",
            &sp,
            "commit",
            "-m",
            "Reset template files to current version",
        ]);
        let (_, stderr, code) = run_git(&["git", "-C", &sp, "push"]);
        if code == 0 {
            println!("  Pushed changes");
        } else {
            println!("  Push failed: {}", stderr.trim());
        }
    } else {
        println!("  Templates already up to date");
    }

    // 4. Update submodule ref in parent
    run_git(&["git", "add", &sp]);

    Ok(())
}

/// corky mailbox reset [NAME] [--no-sync]
pub fn run(name: Option<&str>, no_sync: bool) -> Result<()> {
    let config = corky_config::try_load_config(None);
    let mailbox_names: Vec<String> = config
        .as_ref()
        .map(|c| c.mailboxes.keys().cloned().collect())
        .unwrap_or_default();

    if mailbox_names.is_empty() {
        println!("No mailboxes configured in .corky.toml");
        return Ok(());
    }

    let names: Vec<String> = if let Some(n) = name {
        if !mailbox_names.contains(&n.to_string()) {
            anyhow::bail!("Unknown mailbox: {}", n);
        }
        vec![n.to_string()]
    } else {
        mailbox_names
    };

    let owner = load_owner(None)?;
    let owner_name = if owner.name.is_empty() {
        &owner.github_user
    } else {
        &owner.name
    };

    for n in &names {
        reset_one(n, owner_name, !no_sync)?;
    }

    Ok(())
}
