//! Add a new mailbox: plain directory or shared GitHub repo (submodule).

use anyhow::Result;

use crate::accounts::load_owner;
use crate::resolve;
use crate::util::run_cmd_checked;

use super::templates::{generate_agents_md, generate_readme_md};

#[allow(clippy::too_many_arguments)]
pub fn run(
    name: &str,
    labels: &[String],
    display_name: &str,
    github: bool,
    github_user: &str,
    pat: bool,
    public: bool,
    account: &str,
    org: &str,
) -> Result<()> {
    let mb_name = name.to_lowercase();
    let mb_dir = resolve::mailbox_dir(&mb_name);

    if mb_dir.exists() {
        anyhow::bail!("Mailbox directory {} already exists", mb_dir.display());
    }

    let owner = load_owner(None)?;
    let owner_name = if owner.name.is_empty() {
        &owner.github_user
    } else {
        &owner.name
    };
    let mb_display = if display_name.is_empty() {
        &mb_name
    } else {
        display_name
    };

    if github {
        // Shared mailbox via GitHub submodule
        let gh_user = if github_user.is_empty() {
            &mb_name
        } else {
            github_user
        };
        let org = if org.is_empty() {
            &owner.github_user
        } else {
            org
        };
        let repo_name = format!("to-{}", gh_user.to_lowercase());
        let repo_full = format!("{}/{}", org, repo_name);

        // 1. Create GitHub repo
        let visibility = if public { "--public" } else { "--private" };
        println!(
            "Creating GitHub repo: {} ({})",
            repo_full,
            visibility.trim_start_matches("--")
        );
        run_cmd_checked(&["gh", "repo", "create", &repo_full, visibility, "--confirm"])?;

        // 2. Add collaborator if not --pat
        if !pat {
            println!("Adding {} as collaborator on {}", gh_user, repo_full);
            run_cmd_checked(&[
                "gh",
                "api",
                &format!("repos/{}/collaborators/{}", repo_full, gh_user),
                "-X",
                "PUT",
                "--silent",
            ])?;
        } else {
            println!();
            println!("PAT access mode selected. The collaborator should:");
            println!("  1. Go to https://github.com/settings/personal-access-tokens/new");
            println!("  2. Create a fine-grained PAT scoped to: {}", repo_full);
            println!("  3. Grant 'Contents' read/write permission");
            println!(
                "  4. Use the PAT to clone: https://github.com/{}.git",
                repo_full
            );
            println!();
        }

        // 3. Initialize the shared repo
        println!("Initializing shared repo contents...");

        let tmpdir = tempfile::tempdir()?;
        let tmp = tmpdir.path();

        run_cmd_checked(&["gh", "repo", "clone", &repo_full, &tmp.to_string_lossy()])?;

        // AGENTS.md + CLAUDE.md symlink + README.md
        std::fs::write(
            tmp.join("AGENTS.md"),
            generate_agents_md(mb_display, owner_name),
        )?;
        #[cfg(unix)]
        std::os::unix::fs::symlink("AGENTS.md", tmp.join("CLAUDE.md"))?;
        std::fs::write(
            tmp.join("README.md"),
            generate_readme_md(mb_display, owner_name),
        )?;

        // .gitignore
        std::fs::write(
            tmp.join(".gitignore"),
            "AGENTS.local.md\nCLAUDE.local.md\n__pycache__/\n",
        )?;

        // voice.md
        let voice_file = resolve::voice_md();
        if voice_file.exists() {
            std::fs::copy(&voice_file, tmp.join("voice.md"))?;
        }

        // .claude/skills/corky/
        crate::skill::install_at(Some(tmp))?;

        // directories
        std::fs::create_dir_all(tmp.join("conversations"))?;
        std::fs::write(tmp.join("conversations/.gitkeep"), "")?;
        std::fs::create_dir_all(tmp.join("drafts"))?;
        std::fs::write(tmp.join("drafts/.gitkeep"), "")?;

        // commit and push
        let tmp_str = tmp.to_string_lossy().to_string();
        run_cmd_checked(&["git", "-C", &tmp_str, "add", "-A"])?;
        run_cmd_checked(&[
            "git",
            "-C",
            &tmp_str,
            "commit",
            "-m",
            &format!("Initialize shared mailbox for {}", mb_display),
        ])?;
        run_cmd_checked(&["git", "-C", &tmp_str, "push"])?;

        // 4. Add as git submodule
        let repo_url = format!("git@github.com:{}.git", repo_full);
        let sub_path = mb_dir.to_string_lossy().to_string();
        println!("Adding submodule: {} -> {}", sub_path, repo_url);
        run_cmd_checked(&["git", "submodule", "add", &repo_url, &sub_path])?;
    } else {
        // Plain directory mailbox
        println!("Creating mailbox: {}", mb_dir.display());
        for sub in &["conversations", "drafts", "contacts"] {
            let d = mb_dir.join(sub);
            std::fs::create_dir_all(&d)?;
            std::fs::write(d.join(".gitkeep"), "")?;
        }

        // AGENTS.md + CLAUDE.md symlink + README.md
        std::fs::write(
            mb_dir.join("AGENTS.md"),
            generate_agents_md(mb_display, owner_name),
        )?;
        #[cfg(unix)]
        std::os::unix::fs::symlink("AGENTS.md", mb_dir.join("CLAUDE.md"))?;
        std::fs::write(
            mb_dir.join("README.md"),
            generate_readme_md(mb_display, owner_name),
        )?;

        // voice.md
        let voice_file = resolve::voice_md();
        if voice_file.exists() {
            std::fs::copy(&voice_file, mb_dir.join("voice.md"))?;
        }

        // .claude/skills/corky/
        crate::skill::install_at(Some(&mb_dir))?;
    }

    // 5. Update .corky.toml with routing and mailbox config
    update_config(&mb_name, labels, account)?;

    // 6. Summary
    println!();
    println!("Done! Next steps:");
    for label in labels {
        println!(
            "  - Ensure '{}' is in your account's labels in .corky.toml",
            label
        );
    }
    println!("  - Run: corky sync --full");
    if github {
        println!("  - Run: corky mailbox sync {}", mb_name);
    }

    Ok(())
}

/// Add routing and mailbox entries to .corky.toml.
fn update_config(name: &str, labels: &[String], account: &str) -> Result<()> {
    let config_path = resolve::corky_toml();
    if !config_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    // Add [routing] entries
    let routing = doc
        .entry("routing")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(routing_table) = routing.as_table_mut() {
        for label in labels {
            let label_key = if !account.is_empty() {
                format!("{}:{}", account, label)
            } else {
                label.clone()
            };
            let mb_path = format!("mailboxes/{}", name);

            // Get or create the array for this label
            if let Some(existing) = routing_table.get_mut(&label_key) {
                if let Some(arr) = existing.as_array_mut() {
                    // Don't duplicate
                    let already = arr.iter().any(|v| v.as_str() == Some(&mb_path));
                    if !already {
                        arr.push(&mb_path);
                    }
                }
            } else {
                let mut arr = toml_edit::Array::new();
                arr.push(&mb_path);
                routing_table.insert(&label_key, toml_edit::value(arr));
            }
        }
    }

    // Add [mailboxes.{name}] section
    let mailboxes = doc
        .entry("mailboxes")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(mailboxes_table) = mailboxes.as_table_mut()
        && !mailboxes_table.contains_key(name) {
            let mb_config = toml_edit::Table::new();
            mailboxes_table.insert(name, toml_edit::Item::Table(mb_config));
        }

    std::fs::write(&config_path, doc.to_string())?;
    println!("Updated {}", config_path.display());

    Ok(())
}
