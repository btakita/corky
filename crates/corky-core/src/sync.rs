//! Core sync configuration types shared by `.corky.toml` and the mail crate.

pub mod imports {
    use serde::{Deserialize, Serialize};

    /// A single `[[imports]]` entry in `.corky.toml`.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct ImportConfig {
        /// Import type: `sms`, `telegram`, or `slack`.
        #[serde(rename = "type")]
        pub import_type: String,
        /// Path to the source file (XML, JSON, ZIP, or directory).
        pub path: String,
        /// Label for imported conversations.
        #[serde(default = "default_label")]
        pub label: String,
        /// Account name for imported conversations.
        #[serde(default = "default_account")]
        pub account: String,
    }

    fn default_label() -> String {
        String::new()
    }

    fn default_account() -> String {
        String::new()
    }

    impl ImportConfig {
        /// Resolve the label, falling back to the import type if empty.
        pub fn resolved_label(&self) -> &str {
            if self.label.is_empty() {
                &self.import_type
            } else {
                &self.label
            }
        }

        /// Resolve the account name, falling back to the import type if empty.
        pub fn resolved_account(&self) -> &str {
            if self.account.is_empty() {
                &self.import_type
            } else {
                &self.account
            }
        }
    }
}
