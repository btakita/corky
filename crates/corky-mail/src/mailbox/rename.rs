//! Rename a mailbox directory and config entry.

use anyhow::Result;

use crate::resolve;
use crate::util::run_cmd_checked;

pub fn run(old_name: &str, new_name: &str, rename_repo: bool) -> Result<()> {
    let old_dir = resolve::mailbox_dir(old_name);
    let new_dir = resolve::mailbox_dir(new_name);

    if new_dir.exists() {
        anyhow::bail!(
            "Mailbox '{}' already exists at {}",
            new_name,
            new_dir.display()
        );
    }

    // 1. Move directory via git mv (if it exists)
    if old_dir.exists() {
        println!(
            "Moving {} \u{2192} {}",
            old_dir.display(),
            new_dir.display()
        );

        // Ensure parent directory exists
        if let Some(parent) = new_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }

        run_cmd_checked(&[
            "git",
            "mv",
            &old_dir.to_string_lossy(),
            &new_dir.to_string_lossy(),
        ])?;
    } else {
        println!(
            "Directory for '{}' not found on disk \u{2014} skipping git mv",
            old_name
        );
    }

    // 2. Optionally rename the GitHub repo
    if rename_repo {
        let owner_gh = crate::accounts::load_owner(None)
            .map(|o| o.github_user)
            .unwrap_or_default();
        if !owner_gh.is_empty() {
            let old_repo = format!("{}/to-{}", owner_gh, old_name.to_lowercase());
            let new_repo_name = format!("to-{}", new_name.to_lowercase());
            println!(
                "Renaming GitHub repo {} \u{2192} {}",
                old_repo, new_repo_name
            );
            run_cmd_checked(&[
                "gh",
                "repo",
                "rename",
                &new_repo_name,
                "-R",
                &old_repo,
                "--yes",
            ])?;
        }
    }

    // 3. Update .corky.toml
    update_config(old_name, new_name)?;

    println!("Done. Mailbox '{}' renamed to '{}'.", old_name, new_name);
    Ok(())
}

/// Rename mailbox in .corky.toml: update [mailboxes] key and [routing] paths.
fn update_config(old_name: &str, new_name: &str) -> Result<()> {
    let config_path = resolve::corky_toml();
    if !config_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    // Rename [mailboxes.{old}] → [mailboxes.{new}]
    if let Some(mailboxes) = doc.get_mut("mailboxes")
        && let Some(table) = mailboxes.as_table_mut()
        && let Some(entry) = table.remove(old_name)
    {
        table.insert(new_name, entry);
    }

    // Update routing paths
    let old_path = format!("mailboxes/{}", old_name);
    let new_path = format!("mailboxes/{}", new_name);
    if let Some(routing) = doc.get_mut("routing")
        && let Some(table) = routing.as_table_mut()
    {
        for (_, item) in table.iter_mut() {
            if let Some(arr) = item.as_array_mut() {
                for i in 0..arr.len() {
                    if arr.get(i).and_then(|v| v.as_str()) == Some(&old_path) {
                        arr.replace(i, &new_path);
                    }
                }
            }
        }
    }

    std::fs::write(&config_path, doc.to_string())?;
    println!(
        "Renamed '{}' \u{2192} '{}' in .corky.toml",
        old_name, new_name
    );

    Ok(())
}
