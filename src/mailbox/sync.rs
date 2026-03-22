//! Sync shared mailboxes: pull changes, push updates.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{corky_config, topic};
use crate::resolve;

fn run_git(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(args[0])
        .args(&args[1..])
        .output()
        .unwrap_or_else(|_| {
            panic!("Failed to run: {}", args.join(" "));
        });
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, stderr, code)
}

fn mailbox_status(name: &str, mb_path: &Path) {
    let sp = mb_path.to_string_lossy().to_string();
    run_git(&["git", "-C", &sp, "fetch"]);

    let (incoming, _, inc_code) =
        run_git(&["git", "-C", &sp, "rev-list", "--count", "HEAD..@{u}"]);
    let (outgoing, _, out_code) =
        run_git(&["git", "-C", &sp, "rev-list", "--count", "@{u}..HEAD"]);

    let inc = if inc_code == 0 {
        incoming.trim().to_string()
    } else {
        "?".to_string()
    };
    let out = if out_code == 0 {
        outgoing.trim().to_string()
    } else {
        "?".to_string()
    };

    if inc == "0" && out == "0" {
        println!("  {}: up to date", name);
    } else {
        let mut parts = Vec::new();
        if inc != "0" {
            parts.push(format!("{} incoming", inc));
        }
        if out != "0" {
            parts.push(format!("{} outgoing", out));
        }
        println!("  {}: {}", name, parts.join(", "));
    }
}

/// Check if a directory is a git repo (submodule or standalone).
fn is_git_repo(path: &Path) -> bool {
    // .git file (submodule) or .git directory (standalone repo)
    path.join(".git").exists()
}

/// Copy a file if the source is newer than the destination (or dest missing).
/// Returns true if copied.
fn copy_if_newer(src: &Path, dst: &Path) -> std::io::Result<bool> {
    if !src.exists() {
        return Ok(false);
    }
    let should_copy = if dst.exists() {
        let src_mtime = src.metadata().ok().and_then(|m| m.modified().ok());
        let dst_mtime = dst.metadata().ok().and_then(|m| m.modified().ok());
        match (src_mtime, dst_mtime) {
            (Some(s), Some(d)) => s > d,
            _ => true,
        }
    } else {
        true
    };
    if should_copy {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Collect all files under a directory (relative paths).
fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.is_dir() {
        return files;
    }
    fn walk(dir: &Path, base: &Path, files: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, base, files);
                } else if let Ok(rel) = path.strip_prefix(base) {
                    files.push(rel.to_path_buf());
                }
            }
        }
    }
    walk(dir, dir, &mut files);
    files
}

/// Bidirectional sync of topic directories between root and mailbox.
///
/// For each topic that lists `mailbox_name` in its `mailboxes` field:
/// - Forward: root `topics/{name}/` → `mailboxes/{mailbox}/topics/{name}/`
/// - Reverse: `mailboxes/{mailbox}/topics/{name}/` → root `topics/{name}/`
///
/// Newer file wins (mtime comparison). Missing files are copied.
fn sync_topics(
    mailbox_name: &str,
    mb_path: &Path,
    config_path: Option<&Path>,
    data_dir: Option<&Path>,
) -> Result<()> {
    let topics = topic::topics_for_mailbox(mailbox_name, config_path)?;
    if topics.is_empty() {
        return Ok(());
    }

    let root_topics_dir = data_dir
        .map(|d| d.join("topics"))
        .unwrap_or_else(|| resolve::data_dir().join("topics"));
    let mb_topics_dir = mb_path.join("topics");

    let mut synced = 0u32;
    for topic_name in topics.keys() {
        let root_dir = root_topics_dir.join(topic_name);
        let mb_dir = mb_topics_dir.join(topic_name);

        // Collect files from both sides
        let root_files = collect_files(&root_dir);
        let mb_files = collect_files(&mb_dir);

        // Union of all relative paths
        let mut all_files: Vec<PathBuf> = root_files.clone();
        for f in &mb_files {
            if !all_files.contains(f) {
                all_files.push(f.clone());
            }
        }

        for rel_path in &all_files {
            let root_file = root_dir.join(rel_path);
            let mb_file = mb_dir.join(rel_path);

            // Forward: root → mailbox
            if copy_if_newer(&root_file, &mb_file)? {
                synced += 1;
            }
            // Reverse: mailbox → root
            if copy_if_newer(&mb_file, &root_file)? {
                synced += 1;
            }
        }
    }

    if synced > 0 {
        println!(
            "  Synced {} topic file(s) ({})",
            synced,
            topics.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(())
}

/// Full sync for one mailbox.
pub fn sync_one(name: &str) -> Result<()> {
    let mb_path = resolve::mailbox_dir(name);
    if !mb_path.exists() {
        println!(
            "  {}: mailbox not found at {} -- skipping",
            name,
            mb_path.display()
        );
        return Ok(());
    }

    if !is_git_repo(&mb_path) {
        println!("  {}: plain directory -- skipping git sync", name);
        return Ok(());
    }

    println!("Syncing {}...", name);
    let sp = mb_path.to_string_lossy().to_string();

    // Pull changes (skip if no remote configured)
    let (remotes, _, _) = run_git(&["git", "-C", &sp, "remote"]);
    if remotes.trim().is_empty() {
        // No remote — local-only mailbox, skip pull silently
    } else {
        let (stdout, stderr, code) = run_git(&["git", "-C", &sp, "pull", "--rebase"]);
        if code == 0 {
            if !stdout.contains("Already up to date") {
                println!("  Pulled changes");
            }
        } else {
            let reason = stderr.lines().last().unwrap_or("unknown error").trim();
            println!("  Pull failed ({})", reason);
        }
    }

    // Copy voice.md if root copy is newer
    let voice_file = resolve::voice_md();
    let mb_voice = mb_path.join("voice.md");
    if voice_file.exists() && copy_if_newer(&voice_file, &mb_voice)? {
        println!("  Updated voice.md");
    }

    // Bidirectional topic sync
    sync_topics(name, &mb_path, None, None)?;

    // Stage, commit, push any local changes
    run_git(&["git", "-C", &sp, "add", "-A"]);

    let (status_out, _, _) = run_git(&["git", "-C", &sp, "status", "--porcelain"]);
    if !status_out.trim().is_empty() {
        run_git(&[
            "git",
            "-C",
            &sp,
            "commit",
            "-m",
            "Sync shared conversations",
        ]);
        let (_, stderr, code) = run_git(&["git", "-C", &sp, "push"]);
        if code == 0 {
            println!("  Pushed changes");
        } else {
            println!("  Push failed: {}", stderr.trim());
        }
    } else {
        println!("  No local changes to push");
    }

    // Update submodule ref in parent
    run_git(&["git", "add", &sp]);

    Ok(())
}

/// corky mailbox sync [NAME]
pub fn run(name: Option<&str>) -> Result<()> {
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

    for n in &names {
        sync_one(n)?;
    }

    Ok(())
}

/// corky mailbox status
pub fn status() -> Result<()> {
    let config = corky_config::try_load_config(None);
    let mailbox_names: Vec<String> = config
        .as_ref()
        .map(|c| c.mailboxes.keys().cloned().collect())
        .unwrap_or_default();

    if mailbox_names.is_empty() {
        println!("No mailboxes configured in .corky.toml");
        return Ok(());
    }

    println!("Mailbox status:");
    for name in &mailbox_names {
        let mb_path = resolve::mailbox_dir(name);
        if mb_path.exists() {
            if is_git_repo(&mb_path) {
                mailbox_status(name, &mb_path);
            } else {
                println!("  {}: plain directory", name);
            }
        } else {
            println!("  {}: not found", name);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn copy_if_newer_creates_missing_dst() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("sub/dst.txt");
        fs::write(&src, "hello").unwrap();
        assert!(copy_if_newer(&src, &dst).unwrap());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "hello");
    }

    #[test]
    fn copy_if_newer_skips_older_src() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        fs::write(&src, "old").unwrap();
        thread::sleep(Duration::from_millis(50));
        fs::write(&dst, "new").unwrap();
        assert!(!copy_if_newer(&src, &dst).unwrap());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "new");
    }

    #[test]
    fn copy_if_newer_overwrites_older_dst() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst.txt");
        let src = dir.path().join("src.txt");
        fs::write(&dst, "old").unwrap();
        thread::sleep(Duration::from_millis(50));
        fs::write(&src, "updated").unwrap();
        assert!(copy_if_newer(&src, &dst).unwrap());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "updated");
    }

    #[test]
    fn copy_if_newer_nonexistent_src() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("nope.txt");
        let dst = dir.path().join("dst.txt");
        assert!(!copy_if_newer(&src, &dst).unwrap());
    }

    #[test]
    fn collect_files_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("topics/test");
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("README.md"), "# Test").unwrap();
        fs::write(base.join("sub/notes.md"), "notes").unwrap();

        let mut files = collect_files(&base);
        files.sort();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], PathBuf::from("README.md"));
        assert_eq!(files[1], PathBuf::from("sub/notes.md"));
    }

    #[test]
    fn collect_files_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("empty");
        fs::create_dir_all(&base).unwrap();
        let files = collect_files(&base);
        assert!(files.is_empty());
    }

    #[test]
    fn collect_files_nonexistent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("nope");
        let files = collect_files(&base);
        assert!(files.is_empty());
    }

    #[test]
    fn sync_topics_forward_copies_to_mailbox() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();

        let root_topic = data.join("topics/brian-takita");
        fs::create_dir_all(&root_topic).unwrap();
        fs::write(root_topic.join("README.md"), "# Brian Takita").unwrap();

        let mb_path = data.join("mailboxes/lucas");
        fs::create_dir_all(&mb_path).unwrap();

        let config_path = data.join(".corky.toml");
        fs::write(
            &config_path,
            r#"[topics.brian-takita]
keywords = ["corky"]
mailboxes = ["lucas"]
"#,
        )
        .unwrap();

        sync_topics("lucas", &mb_path, Some(&config_path), Some(data)).unwrap();

        let mb_readme = mb_path.join("topics/brian-takita/README.md");
        assert!(mb_readme.exists());
        assert_eq!(fs::read_to_string(&mb_readme).unwrap(), "# Brian Takita");
    }

    #[test]
    fn sync_topics_reverse_copies_to_root() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();

        let root_topic = data.join("topics/brian-takita");
        fs::create_dir_all(&root_topic).unwrap();
        fs::write(root_topic.join("README.md"), "old content").unwrap();

        thread::sleep(Duration::from_millis(50));

        let mb_path = data.join("mailboxes/lucas");
        let mb_topic = mb_path.join("topics/brian-takita");
        fs::create_dir_all(&mb_topic).unwrap();
        fs::write(mb_topic.join("README.md"), "lucas edit").unwrap();

        let config_path = data.join(".corky.toml");
        fs::write(
            &config_path,
            r#"[topics.brian-takita]
keywords = ["corky"]
mailboxes = ["lucas"]
"#,
        )
        .unwrap();

        sync_topics("lucas", &mb_path, Some(&config_path), Some(data)).unwrap();

        assert_eq!(
            fs::read_to_string(root_topic.join("README.md")).unwrap(),
            "lucas edit"
        );
    }

    #[test]
    fn sync_topics_no_matching_topics() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();

        let mb_path = data.join("mailboxes/lucas");
        fs::create_dir_all(&mb_path).unwrap();

        let config_path = data.join(".corky.toml");
        fs::write(
            &config_path,
            r#"[topics.brian-takita]
keywords = ["corky"]
mailboxes = ["alice"]
"#,
        )
        .unwrap();

        sync_topics("lucas", &mb_path, Some(&config_path), Some(data)).unwrap();
        assert!(!mb_path.join("topics").exists());
    }

    #[test]
    fn sync_topics_bidirectional_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();

        let root_topic = data.join("topics/shared");
        fs::create_dir_all(&root_topic).unwrap();
        fs::write(root_topic.join("from-root.md"), "root content").unwrap();

        let mb_path = data.join("mailboxes/lucas");
        let mb_topic = mb_path.join("topics/shared");
        fs::create_dir_all(&mb_topic).unwrap();
        fs::write(mb_topic.join("from-lucas.md"), "lucas content").unwrap();

        let config_path = data.join(".corky.toml");
        fs::write(
            &config_path,
            r#"[topics.shared]
keywords = []
mailboxes = ["lucas"]
"#,
        )
        .unwrap();

        sync_topics("lucas", &mb_path, Some(&config_path), Some(data)).unwrap();

        assert!(root_topic.join("from-lucas.md").exists());
        assert!(mb_topic.join("from-root.md").exists());
        assert_eq!(
            fs::read_to_string(root_topic.join("from-lucas.md")).unwrap(),
            "lucas content"
        );
        assert_eq!(
            fs::read_to_string(mb_topic.join("from-root.md")).unwrap(),
            "root content"
        );
    }
}
