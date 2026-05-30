//! Config-driven import dispatcher.
//!
//! Reads `[[imports]]` entries from `.corky.toml` and dispatches each to the
//! appropriate handler (`sms_import`, `telegram_import`, `slack_import`).

use anyhow::{Result, bail};
use std::path::Path;

use crate::resolve;
pub use corky_core::sync::imports::ImportConfig;

/// Expand `~` in an import path to the user's home directory.
pub fn resolved_path(import: &ImportConfig) -> std::path::PathBuf {
    resolve::expand_tilde(&import.path)
}

/// Run all configured imports.
pub fn run(imports: &[ImportConfig], data_dir: &Path) -> Result<()> {
    if imports.is_empty() {
        println!("No [[imports]] configured in .corky.toml.");
        return Ok(());
    }

    let out_dir = data_dir.join("conversations");

    for (i, import) in imports.iter().enumerate() {
        println!(
            "\n--- Import {}/{}: {} ({}) ---",
            i + 1,
            imports.len(),
            import.import_type,
            import.path
        );

        let path = resolved_path(import);
        if !path.exists() {
            eprintln!("  Warning: path not found: {}", path.display());
            continue;
        }

        let label = import.resolved_label();
        let account = import.resolved_account();

        match import.import_type.as_str() {
            "sms" => super::sms_import::run(&path, label, &out_dir, account)?,
            "telegram" => super::telegram_import::run(&path, label, &out_dir, account)?,
            "slack" => super::slack_import::run(&path, label, &out_dir, account)?,
            other => bail!(
                "Unknown import type '{}'. Expected: sms, telegram, slack.",
                other
            ),
        }
    }

    println!("\nAll imports complete.");
    Ok(())
}

/// Load `[[imports]]` from `.corky.toml` and run them.
pub fn run_from_config() -> Result<()> {
    let config = crate::config::corky_config::load_config(None)?;
    let data_dir = resolve::data_dir();
    run(&config.imports, &data_dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn test_import_config_deserialize() {
        let toml_str = r#"
[[imports]]
type = "sms"
path = "~/Downloads/sms-backup.xml"
label = "sms"
account = "personal"

[[imports]]
type = "telegram"
path = "/tmp/telegram-export"

[[imports]]
type = "slack"
path = "/tmp/slack-export.zip"
label = "work-slack"
account = "work"
"#;

        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            imports: Vec<ImportConfig>,
        }

        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(wrapper.imports.len(), 3);

        assert_eq!(wrapper.imports[0].import_type, "sms");
        assert_eq!(wrapper.imports[0].path, "~/Downloads/sms-backup.xml");
        assert_eq!(wrapper.imports[0].label, "sms");
        assert_eq!(wrapper.imports[0].account, "personal");

        assert_eq!(wrapper.imports[1].import_type, "telegram");
        assert_eq!(wrapper.imports[1].resolved_label(), "telegram");
        assert_eq!(wrapper.imports[1].resolved_account(), "telegram");

        assert_eq!(wrapper.imports[2].import_type, "slack");
        assert_eq!(wrapper.imports[2].label, "work-slack");
    }

    #[test]
    fn test_resolved_path_tilde_expansion() {
        let cfg = ImportConfig {
            import_type: "sms".into(),
            path: "~/Downloads/sms.xml".into(),
            label: String::new(),
            account: String::new(),
        };
        let resolved = resolved_path(&cfg);
        // Should not start with ~ after expansion
        assert!(!resolved.to_string_lossy().starts_with('~'));
        assert!(resolved.to_string_lossy().ends_with("Downloads/sms.xml"));
    }

    #[test]
    fn test_resolved_defaults() {
        let cfg = ImportConfig {
            import_type: "telegram".into(),
            path: "/tmp/export".into(),
            label: String::new(),
            account: String::new(),
        };
        assert_eq!(cfg.resolved_label(), "telegram");
        assert_eq!(cfg.resolved_account(), "telegram");
    }

    #[test]
    fn test_run_empty_imports() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(&[], dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_skips_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let imports = vec![ImportConfig {
            import_type: "sms".into(),
            path: "/nonexistent/path/sms.xml".into(),
            label: "sms".into(),
            account: "test".into(),
        }];
        // Should not error — just prints a warning and skips
        let result = run(&imports, dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_unknown_type_errors() {
        let dir = tempfile::tempdir().unwrap();
        // Create a dummy file so the path exists
        let dummy = dir.path().join("dummy.txt");
        std::fs::write(&dummy, "").unwrap();

        let imports = vec![ImportConfig {
            import_type: "whatsapp".into(),
            path: dummy.to_string_lossy().into(),
            label: "wa".into(),
            account: "test".into(),
        }];
        let result = run(&imports, dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("whatsapp"));
    }
}
