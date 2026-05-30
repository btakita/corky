use crate::accounts::{load_accounts, resolve_password};
use crate::sync::imap_sync::connect_imap_pub;
use anyhow::{Context, Result};

/// Remove a Gmail/IMAP label from all messages (or those matching a search query).
///
/// For Gmail: selecting a label folder, marking messages \Deleted, and expunging
/// removes the label without deleting the messages themselves.
pub fn run(label: &str, account: Option<&str>, search: Option<&str>, dry_run: bool) -> Result<()> {
    let accounts = load_accounts(None)?;

    let target_accounts: Vec<(&str, &crate::accounts::Account)> = if let Some(name) = account {
        let acct = accounts
            .get(name)
            .with_context(|| format!("Account '{}' not found in .corky.toml", name))?;
        vec![(name, acct)]
    } else {
        // Use all accounts
        accounts.iter().map(|(k, v)| (k.as_str(), v)).collect()
    };

    for (acct_name, acct) in &target_accounts {
        let password = resolve_password(acct)?;
        println!(
            "Connecting to {}:{} as {}",
            acct.imap_host, acct.imap_port, acct.user
        );

        let mut session = connect_imap_pub(
            &acct.imap_host,
            acct.imap_port,
            acct.imap_starttls,
            &acct.user,
            &password,
        )?;

        // Select the label folder
        match session.select(label) {
            Ok(_) => {}
            Err(_) => {
                println!(
                    "  Label \"{}\" not found on account '{}' \u{2014} skipping",
                    label, acct_name
                );
                continue;
            }
        }

        // Search for messages
        let query = search.unwrap_or("ALL");
        let uids: Vec<u32> = session
            .uid_search(query)
            .with_context(|| format!("IMAP search failed for query: {}", query))?
            .into_iter()
            .collect();

        if uids.is_empty() {
            println!(
                "  No messages found in \"{}\" (account: {})",
                label, acct_name
            );
            let _ = session.logout();
            continue;
        }

        if dry_run {
            println!(
                "  [dry-run] Would clear label \"{}\" from {} message(s) (account: {})",
                label,
                uids.len(),
                acct_name
            );
            let _ = session.logout();
            continue;
        }

        // Process in batches of 500 UIDs to avoid IMAP command length limits
        let batch_size = 500;
        let total = uids.len();
        let mut cleared = 0;

        for chunk in uids.chunks(batch_size) {
            let uid_set = chunk
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(",");

            session
                .uid_store(&uid_set, "+FLAGS (\\Deleted)")
                .with_context(|| "Failed to mark messages as deleted")?;

            cleared += chunk.len();
            if total > batch_size {
                println!("  Progress: {}/{}", cleared, total);
            }
        }

        session.expunge().context("EXPUNGE failed")?;

        println!(
            "  Cleared label \"{}\" from {} message(s) (account: {})",
            label, total, acct_name
        );

        let _ = session.logout();
    }

    Ok(())
}
