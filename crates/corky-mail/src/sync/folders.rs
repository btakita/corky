//! List IMAP folders for a configured account.

use anyhow::Result;

use crate::accounts::{load_accounts, resolve_password};

pub fn run(account: Option<&str>) -> Result<()> {
    let accounts = load_accounts(None)?;

    let Some(account_name) = account else {
        println!("Available accounts:");
        for (name, acct) in &accounts {
            println!("  {:<20} {}", name, acct.user);
        }
        return Ok(());
    };

    let acct = accounts.get(account_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown account: {}\nAvailable: {}",
            account_name,
            accounts.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    let password = resolve_password(acct)?;

    let mut tls_builder = native_tls::TlsConnector::builder();
    if acct.imap_starttls || acct.imap_host == "127.0.0.1" || acct.imap_host == "localhost" {
        tls_builder.danger_accept_invalid_certs(true);
        tls_builder.danger_accept_invalid_hostnames(true);
    }
    let tls = tls_builder.build()?;

    println!(
        "Connecting to {}:{} as {}\n",
        acct.imap_host, acct.imap_port, acct.user
    );

    let client = if acct.imap_starttls {
        imap::connect_starttls(
            (acct.imap_host.as_str(), acct.imap_port),
            &acct.imap_host,
            &tls,
        )?
    } else {
        imap::connect(
            (acct.imap_host.as_str(), acct.imap_port),
            &acct.imap_host,
            &tls,
        )?
    };

    let mut session = client.login(&acct.user, &password).map_err(|e| e.0)?;
    let folders = session.list(None, Some("*"))?;

    for folder in folders.iter() {
        let flags: Vec<String> = folder
            .attributes()
            .iter()
            .map(|a| format!("{:?}", a))
            .collect();
        println!("  {:<40} [{}]", folder.name(), flags.join(", "));
    }

    session.logout()?;
    Ok(())
}
