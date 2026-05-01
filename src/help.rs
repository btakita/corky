//! Command reference for corky.

use anyhow::Result;

const COMMANDS: &[(&str, &str)] = &[
    (
        "init --user EMAIL [PATH]",
        "Initialize a new project directory",
    ),
    ("install-skill NAME", "Install an agent skill (e.g. email)"),
    ("sync", "Incremental IMAP sync (default)"),
    ("sync full", "Full IMAP resync"),
    ("sync account NAME", "Sync one account"),
    ("sync routes", "Apply routing to existing conversations"),
    ("sync mailbox [NAME]", "Push/pull shared mailboxes"),
    ("sync-auth", "Gmail OAuth setup"),
    ("list-folders [ACCOUNT]", "List IMAP folders for an account"),
    ("draft new SUBJECT --to EMAIL", "Scaffold a new draft file"),
    (
        "draft validate [FILE|SCOPE...]",
        "Validate draft markdown files",
    ),
    ("draft push FILE [--send]", "Save draft to email"),
    (
        "add-label LABEL --account NAME",
        "Add a label to an account's sync config",
    ),
    (
        "contact add NAME --email EMAIL",
        "Add a contact with context docs",
    ),
    (
        "contact add --from SLUG",
        "Create contact from a conversation",
    ),
    ("contact info NAME", "Show contact info and thread history"),
    ("watch [--interval N]", "Poll IMAP and sync on an interval"),
    (
        "unanswered [SCOPE] [--from NAME]",
        "Find threads awaiting a reply",
    ),
    ("audit-docs", "Audit instruction files"),
    ("migrate", "Migrate from accounts.toml to .corky.toml"),
    ("help", "Show this reference"),
];

const MAILBOX_COMMANDS: &[(&str, &str)] = &[
    ("mailbox list", "List registered mailboxes"),
    (
        "mailbox add NAME --label LABEL",
        "Add a mailbox (--github for shared repo)",
    ),
    ("mailbox sync [NAME]", "Push/pull shared mailboxes"),
    ("mailbox status", "Check for pending changes"),
    ("mailbox remove NAME [--delete-repo]", "Remove a mailbox"),
    ("mailbox rename OLD NEW", "Rename a mailbox"),
    (
        "mailbox draft new SUBJECT --to EMAIL",
        "Scaffold a new draft file (scoped)",
    ),
    (
        "mailbox draft validate [FILE|SCOPE]",
        "Validate draft files (scoped)",
    ),
    ("mailbox draft push FILE [--send]", "Push a draft (scoped)"),
    (
        "mailbox reset [NAME] [--no-sync]",
        "Pull, regenerate templates, commit & push",
    ),
];

const DEV_COMMANDS: &[(&str, &str)] = &[
    ("cargo test", "Run tests"),
    ("cargo clippy", "Lint"),
    ("cargo fmt", "Format"),
];

pub fn run(filter: Option<&str>) -> Result<()> {
    if let Some(filter) = filter
        && filter != "--dev"
    {
        let all_cmds: Vec<(&str, &str)> = COMMANDS
            .iter()
            .chain(MAILBOX_COMMANDS.iter())
            .chain(DEV_COMMANDS.iter())
            .copied()
            .collect();
        let matches: Vec<_> = all_cmds
            .iter()
            .filter(|(name, _)| name.contains(filter))
            .collect();
        if matches.is_empty() {
            println!("No command matching '{}'", filter);
            std::process::exit(1);
        }
        print_table(&matches.iter().map(|&&(a, b)| (a, b)).collect::<Vec<_>>());
        return Ok(());
    }

    println!("corky commands\n");
    print_table(COMMANDS);

    println!("\nmailbox commands (alias: mb)\n");
    print_table(MAILBOX_COMMANDS);

    if filter == Some("--dev") || filter.is_none() {
        println!("\ndev commands\n");
        print_table(DEV_COMMANDS);
    }

    Ok(())
}

fn print_table(rows: &[(&str, &str)]) {
    let name_w = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (name, desc) in rows {
        println!("  {:<width$}  {}", name, desc, width = name_w);
    }
}
