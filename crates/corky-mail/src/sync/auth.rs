//! Gmail OAuth setup.
//!
//! For now, this prints instructions to use the Python fallback.
//! Full OAuth flow can be implemented later.

use anyhow::Result;

use crate::resolve;

pub fn run() -> Result<()> {
    let creds_file = resolve::credentials_json();
    if !creds_file.exists() {
        anyhow::bail!(
            "credentials.json not found.\n\
             Download it from Google Cloud Console → Clients → your Desktop app → Download JSON\n\
             and save it as credentials.json in the project root."
        );
    }

    println!("Gmail OAuth setup is not yet implemented in the Rust port.");
    println!();
    println!("For now, use the Python version for one-time OAuth setup:");
    println!("  pip install corky==0.6.1");
    println!("  corky sync-auth");
    println!();
    println!("Or use an app password instead:");
    println!("  https://myaccount.google.com/apppasswords");
    println!("  Add password_cmd to accounts.toml");

    Ok(())
}
