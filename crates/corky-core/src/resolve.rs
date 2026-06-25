//! Path resolution for corky data and config directories.
//!
//! Resolution order for data directory:
//!   1. CORKY_DATA environment variable (explicit override: --mailbox, `corky init`, tests)
//!   2. cwd containing .corky.toml (running from inside the mail directory)
//!   3. mail/ in cwd (developer workflow)
//!   4. App config mailbox (via app_config::resolve_mailbox)
//!   5. ~/Documents/mail (general user default)

use std::path::PathBuf;

/// Return the data directory path.
pub fn data_dir() -> PathBuf {
    // CORKY_DATA is an explicit override (set by --mailbox, `corky init`, and
    // tests) and must win over implicit CWD/app-config discovery. Checking it
    // first also keeps tests isolated from a stray ./mail symlink or a real
    // mailbox in app config (#ckysuite).
    if let Ok(env) = std::env::var("CORKY_DATA")
        && !env.is_empty()
    {
        return PathBuf::from(env);
    }
    // If CWD itself contains .corky.toml, treat CWD as the data dir
    // (handles running from inside the mail/ directory).
    let cwd = PathBuf::from(".");
    if cwd.join(".corky.toml").exists() || cwd.join("corky.toml").exists() {
        return cwd;
    }
    let local = PathBuf::from("mail");
    if local.is_dir() {
        return local;
    }
    // Try app config mailbox.
    if let Ok(Some(mailbox_path)) = crate::app_config::resolve_mailbox(None) {
        return mailbox_path;
    }
    home_dir().join("Documents").join("mail")
}

/// Return the config directory path.
///
/// Config always lives inside the data directory (mail/).
pub fn config_dir() -> PathBuf {
    data_dir()
}

// --- Derived helpers: data paths ---

pub fn conversations_dir() -> PathBuf {
    data_dir().join("conversations")
}

pub fn drafts_dir() -> PathBuf {
    data_dir().join("drafts")
}

pub fn contacts_dir() -> PathBuf {
    data_dir().join("contacts")
}

pub fn mailboxes_base_dir() -> PathBuf {
    data_dir().join("mailboxes")
}

pub fn mailbox_dir(name: &str) -> PathBuf {
    mailboxes_base_dir().join(name.to_lowercase())
}

pub fn templates_dir() -> PathBuf {
    data_dir().join("templates")
}

pub fn sync_state_file() -> PathBuf {
    data_dir().join(".sync-state.json")
}

pub fn manifest_file() -> PathBuf {
    data_dir().join("manifest.toml")
}

// --- Derived helpers: config paths ---

/// Resolve .corky.toml path: check .corky.toml then corky.toml in config_dir().
pub fn corky_toml() -> PathBuf {
    let dir = config_dir();
    let dotfile = dir.join(".corky.toml");
    if dotfile.exists() {
        return dotfile;
    }
    let plain = dir.join("corky.toml");
    if plain.exists() {
        return plain;
    }
    // Default to .corky.toml (for creation)
    dotfile
}

pub fn voice_md() -> PathBuf {
    config_dir().join("voice.md")
}

pub fn credentials_json() -> PathBuf {
    config_dir().join("credentials.json")
}

pub fn social_dir() -> PathBuf {
    data_dir().join("social")
}

pub fn profiles_toml() -> PathBuf {
    data_dir().join("profiles.toml")
}

/// Get the user's home directory.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Expand ~ to home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home_dir().join(rest)
    } else if path == "~" {
        home_dir()
    } else {
        PathBuf::from(path)
    }
}
