//! List registered mailboxes from app config.

use anyhow::Result;

use crate::app_config;

pub fn run() -> Result<()> {
    let mailboxes = app_config::list_mailboxes()?;

    if mailboxes.is_empty() {
        println!("No mailboxes configured.");
        println!("Run 'corky init --user EMAIL' to create one.");
        return Ok(());
    }

    println!("corky mailboxes\n");
    let name_w = mailboxes.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    for (name, path, is_default) in &mailboxes {
        let marker = if *is_default { " (default)" } else { "" };
        println!("  {:<width$}  {}{}", name, path, marker, width = name_w);
    }
    Ok(())
}
