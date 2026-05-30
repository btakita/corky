//! Remove a mailbox: plain directory or submodule.

use anyhow::Result;
use std::io::Write;
use std::path::PathBuf;

use crate::resolve;
use crate::util::run_cmd_checked;

pub fn run(name: &str, delete_repo: bool) -> Result<()> {
    let mb_path = resolve::mailbox_dir(name);

    if mb_path.exists() {
        let is_submodule = mb_path.join(".git").is_file();

        if is_submodule {
            // Submodule removal
            println!("Removing submodule: {}", mb_path.display());
            let sp = mb_path.to_string_lossy().to_string();
            run_cmd_checked(&["git", "submodule", "deinit", "-f", &sp])?;
            run_cmd_checked(&["git", "rm", "-f", &sp])?;

            // Clean up .git/modules entry
            let modules_path =
                PathBuf::from(".git/modules").join(mb_path.to_string_lossy().as_ref());
            if modules_path.exists() {
                std::fs::remove_dir_all(&modules_path)?;
                println!("  Cleaned up {}", modules_path.display());
            }
        } else {
            // Plain directory removal
            println!("Removing directory: {}", mb_path.display());
            std::fs::remove_dir_all(&mb_path)?;
        }
    } else {
        println!(
            "Mailbox {} not found on disk -- skipping cleanup",
            mb_path.display()
        );
    }

    // Remove from .corky.toml
    remove_from_config(name)?;

    // Optionally delete GitHub repo
    if delete_repo {
        // Try to find repo name from config or convention
        let owner = crate::accounts::load_owner(None).ok();
        let repo_full = owner
            .map(|o| format!("{}/to-{}", o.github_user, name.to_lowercase()))
            .unwrap_or_default();

        if !repo_full.is_empty() {
            print!(
                "Delete GitHub repo {}? This cannot be undone. [y/N] ",
                repo_full
            );
            std::io::stdout().flush()?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if input.trim().to_lowercase() == "y" {
                run_cmd_checked(&["gh", "repo", "delete", &repo_full, "--yes"])?;
                println!("Deleted GitHub repo: {}", repo_full);
            } else {
                println!("Skipped repo deletion");
            }
        }
    }

    println!("Done. Mailbox '{}' removed.", name);
    Ok(())
}

/// Remove mailbox and routing entries from .corky.toml.
fn remove_from_config(name: &str) -> Result<()> {
    let config_path = resolve::corky_toml();
    if !config_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    // Remove [mailboxes.{name}]
    if let Some(mailboxes) = doc.get_mut("mailboxes")
        && let Some(table) = mailboxes.as_table_mut()
    {
        table.remove(name);
    }

    // Remove routing entries that point to this mailbox
    let mb_path = format!("mailboxes/{}", name);
    if let Some(routing) = doc.get_mut("routing")
        && let Some(table) = routing.as_table_mut()
    {
        let keys_to_check: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
        for key in keys_to_check {
            if let Some(item) = table.get_mut(&key)
                && let Some(arr) = item.as_array_mut()
            {
                arr.retain(|v| v.as_str() != Some(&mb_path));
                if arr.is_empty() {
                    table.remove(&key);
                }
            }
        }
    }

    std::fs::write(&config_path, doc.to_string())?;
    println!("Removed '{}' from .corky.toml", name);

    Ok(())
}
