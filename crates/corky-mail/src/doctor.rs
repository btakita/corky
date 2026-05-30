//! `corky doctor` — health checks for config, accounts, and tokens.

use anyhow::Result;
use serde::Serialize;

use crate::config::corky_config::{self, CorkyConfig};
use crate::filter::gmail_auth::{DEFAULT_GCP_CLIENT_ID, GMAIL_SEND_SCOPE, GMAIL_SYNC_SCOPE};
use crate::resolve;
use crate::social::token_store::{StoredToken, TokenStore, tokens_path};

const GMAIL_FILTER_REQUIRED_SCOPE: &str = "https://www.googleapis.com/auth/gmail.settings.basic https://www.googleapis.com/auth/gmail.labels";

/// Collected diagnostic output and status.
#[derive(Debug, Default)]
pub struct DoctorReport {
    pub lines: Vec<String>,
    pub ok: bool,
}

#[derive(Debug, Serialize)]
struct JsonDoctorReport {
    ok: bool,
    lines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gmail: Option<GmailConnectorStatus>,
}

#[derive(Debug, Serialize)]
struct GmailConnectorStatus {
    credentials_present: bool,
    credential_source: String,
    tokens_path: String,
    default_token: TokenStatus,
    gmail_api_accounts: Vec<GmailAccountStatus>,
}

#[derive(Debug, Serialize)]
struct GmailAccountStatus {
    account_name: String,
    user: String,
    sync_token: TokenStatus,
    send_token: TokenStatus,
}

#[derive(Debug, Serialize)]
struct TokenStatus {
    key: String,
    present: bool,
    valid: bool,
    expires_at: Option<String>,
    has_refresh_token: bool,
    scopes: Vec<String>,
    required_scopes: Vec<String>,
    scope_coverage: bool,
    reauth_required: bool,
}

impl DoctorReport {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            ok: true,
        }
    }

    fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn fail(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
        self.ok = false;
    }
}

/// Run health checks, optionally filtering to a specific provider.
pub fn run(provider: Option<&str>, json: bool) -> Result<()> {
    let report = check(provider);
    if json {
        let payload = JsonDoctorReport {
            ok: report.ok,
            lines: report.lines.clone(),
            gmail: if provider.is_none()
                || provider == Some("gmail")
                || provider == Some("gmail-api")
            {
                let cfg = corky_config::try_load_config(None);
                let store = TokenStore::load().unwrap_or_default();
                cfg.as_ref()
                    .map(|cfg| build_gmail_connector_status(cfg, &store))
            } else {
                None
            },
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for line in &report.lines {
            println!("{}", line);
        }
    }
    if !report.ok {
        std::process::exit(1);
    }
    Ok(())
}

/// Run health checks and return a report (testable, no process::exit).
pub fn check(provider: Option<&str>) -> DoctorReport {
    let mut report = DoctorReport::new();

    // --- Config check ---
    let toml_path = resolve::corky_toml();
    let toml_abs = std::fs::canonicalize(&toml_path).unwrap_or_else(|_| toml_path.clone());
    let cfg = match corky_config::load_config(None) {
        Ok(c) => {
            report.push(format!("\u{2713} Config loaded: {}", toml_abs.display()));
            Some(c)
        }
        Err(e) => {
            report.fail(format!(
                "\u{2717} Config not found: {} ({})",
                toml_abs.display(),
                e
            ));
            None
        }
    };

    // --- Mail directory check ---
    let data_dir = resolve::data_dir();
    let data_abs = std::fs::canonicalize(&data_dir).unwrap_or_else(|_| data_dir.clone());
    if data_dir.is_dir() {
        report.push(format!("\u{2713} Mail directory: {}", data_abs.display()));
    } else {
        report.fail(format!(
            "\u{2717} Mail directory missing: {}",
            data_abs.display()
        ));
    }

    let Some(cfg) = cfg else {
        return report;
    };

    // Load token store once for all checks.
    let store = TokenStore::load().unwrap_or_default();

    // --- Per-account checks (email providers) ---
    if provider.is_none()
        || matches!(
            provider,
            Some("gmail") | Some("gmail-api") | Some("imap") | Some("protonmail-bridge")
        )
    {
        check_email_accounts(&cfg, provider, &store, &mut report);
    }

    // --- Gmail filter auth check ---
    if provider.is_none() || provider == Some("gmail") || provider == Some("gmail-api") {
        check_gmail_filters(&cfg, &store, &mut report);
    }

    // --- LinkedIn checks ---
    if provider.is_none() || provider == Some("linkedin") {
        check_linkedin(&cfg, &store, &mut report);
    }

    // --- YouTube checks ---
    if provider.is_none() || provider == Some("youtube") {
        check_youtube(&cfg, &store, &mut report);
    }

    report
}

/// Check email accounts (gmail-api, gmail/imap/protonmail-bridge).
fn check_email_accounts(
    cfg: &CorkyConfig,
    provider: Option<&str>,
    store: &TokenStore,
    report: &mut DoctorReport,
) {
    let mut accounts: Vec<_> = cfg.accounts.iter().collect();
    accounts.sort_by_key(|(name, _)| (*name).clone());

    for (name, account) in &accounts {
        // Filter by provider if requested.
        if let Some(p) = provider
            && account.provider != p
        {
            continue;
        }

        report.push(String::new());
        report.push(format!("Account: {} ({})", name, account.provider));

        match account.provider.as_str() {
            "gmail-api" => {
                check_gmail_api_account(name, account, store, cfg, report);
            }
            "gmail" | "imap" | "protonmail-bridge" => {
                check_imap_account(account, report);
            }
            other => {
                report.push(format!("  ? Unknown provider: {}", other));
            }
        }
    }
}

/// Check Gmail filter auth section.
fn check_gmail_filters(cfg: &CorkyConfig, store: &TokenStore, report: &mut DoctorReport) {
    if cfg.gmail.is_some() {
        report.push(String::new());
        report.push("Gmail Filters:".to_string());
        let key = "gmail:default";
        check_token_status(store, key, "filter auth", "corky filter auth", report);
    }
}

/// Check LinkedIn config and token status.
fn check_linkedin(cfg: &CorkyConfig, store: &TokenStore, report: &mut DoctorReport) {
    // Check if any LinkedIn config exists (top-level or in profiles).
    let has_top_level = cfg.linkedin.is_some();
    let profiles_with_linkedin: Vec<(&String, &crate::social::profiles::PlatformEntry)> = cfg
        .profiles
        .iter()
        .filter_map(|(name, profile)| profile.linkedin.as_ref().map(|entry| (name, entry)))
        .collect();

    if !has_top_level && profiles_with_linkedin.is_empty() {
        return;
    }

    report.push(String::new());
    report.push("LinkedIn:".to_string());

    // OAuth client credentials check.
    if let Some(li) = &cfg.linkedin {
        let has_config_creds = !li.client_id.is_empty()
            || !li.client_id_cmd.is_empty()
            || !li.client_secret.is_empty()
            || !li.client_secret_cmd.is_empty();
        let has_env_creds = std::env::var("CORKY_LINKEDIN_CLIENT_ID")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

        if has_config_creds {
            report.push("  \u{2713} OAuth credentials configured (config)".to_string());
        } else if has_env_creds {
            report.push("  \u{2713} OAuth credentials configured (env)".to_string());
        } else {
            report.fail(
                "  \u{2717} No OAuth credentials \u{2014} set [linkedin] in .corky.toml or CORKY_LINKEDIN_CLIENT_ID env"
                    .to_string(),
            );
        }
    }

    // Per-profile token checks.
    if profiles_with_linkedin.is_empty() {
        report.fail(
            "  \u{2717} No LinkedIn profiles configured \u{2014} add [profiles.<name>.linkedin] to .corky.toml"
                .to_string(),
        );
    } else {
        for (profile_name, entry) in &profiles_with_linkedin {
            if let Some(urn) = &entry.urn {
                report.push(format!(
                    "  \u{2713} Profile '{}': handle={}, urn={}",
                    profile_name, entry.handle, urn
                ));
                check_token_status(store, urn, "LinkedIn token", "corky linkedin auth", report);
            } else {
                report.fail(format!(
                    "  \u{2717} Profile '{}': handle={}, no URN \u{2014} run `corky linkedin auth --profile {}`",
                    profile_name, entry.handle, profile_name
                ));
            }
        }
    }
}

/// Check YouTube config and token status.
fn check_youtube(cfg: &CorkyConfig, store: &TokenStore, report: &mut DoctorReport) {
    // Check if any YouTube config exists (top-level or in profiles).
    let has_top_level = cfg.youtube.is_some();
    let profiles_with_youtube: Vec<(&String, &crate::social::profiles::PlatformEntry)> = cfg
        .profiles
        .iter()
        .filter_map(|(name, profile)| profile.youtube.as_ref().map(|entry| (name, entry)))
        .collect();

    if !has_top_level && profiles_with_youtube.is_empty() {
        return;
    }

    report.push(String::new());
    report.push("YouTube:".to_string());

    // OAuth client credentials check.
    if let Some(yt) = &cfg.youtube {
        let has_config_creds = !yt.client_id.is_empty()
            || !yt.client_id_cmd.is_empty()
            || !yt.client_secret.is_empty()
            || !yt.client_secret_cmd.is_empty();
        let has_env_creds = std::env::var("CORKY_YOUTUBE_CLIENT_ID")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

        if has_config_creds {
            report.push("  \u{2713} OAuth credentials configured (config)".to_string());
        } else if has_env_creds {
            report.push("  \u{2713} OAuth credentials configured (env)".to_string());
        } else {
            report.fail(
                "  \u{2717} No OAuth credentials \u{2014} set [youtube] in .corky.toml or CORKY_YOUTUBE_CLIENT_ID env"
                    .to_string(),
            );
        }
    }

    // Per-profile token checks.
    if profiles_with_youtube.is_empty() {
        report.fail(
            "  \u{2717} No YouTube profiles configured \u{2014} add [profiles.<name>.youtube] to .corky.toml"
                .to_string(),
        );
    } else {
        for (profile_name, entry) in &profiles_with_youtube {
            if let Some(urn) = &entry.urn {
                report.push(format!(
                    "  \u{2713} Profile '{}': handle={}, channel={}",
                    profile_name, entry.handle, urn
                ));
                check_token_status(store, urn, "YouTube token", "corky youtube auth", report);
            } else {
                report.fail(format!(
                    "  \u{2717} Profile '{}': handle={}, no channel ID \u{2014} run `corky youtube auth --profile {}`",
                    profile_name, entry.handle, profile_name
                ));
            }
        }
    }
}

fn check_gmail_api_account(
    name: &str,
    account: &crate::accounts::Account,
    store: &TokenStore,
    cfg: &CorkyConfig,
    report: &mut DoctorReport,
) {
    // Credential resolution check
    let has_custom_creds = cfg.gmail.as_ref().is_some_and(|g| {
        !g.client_id.is_empty()
            || !g.client_id_cmd.is_empty()
            || !g.client_secret.is_empty()
            || !g.client_secret_cmd.is_empty()
    });
    let has_env_creds = std::env::var("CORKY_GMAIL_CLIENT_ID")
        .map(|v| !v.is_empty())
        .unwrap_or(false);

    if has_custom_creds {
        report.push("  \u{2713} OAuth credentials resolved (config)".to_string());
    } else if has_env_creds {
        report.push("  \u{2713} OAuth credentials resolved (env)".to_string());
    } else {
        report.push("  \u{2713} OAuth credentials resolved (built-in defaults)".to_string());
    }

    let sync_key = format!("gmail:{}", account.user);
    check_token_status_with_scope(
        store,
        &sync_key,
        "sync token",
        "corky sync account",
        GMAIL_SYNC_SCOPE,
        report,
    );
    let send_key = format!("gmail:{}:send", name);
    check_token_status_with_scope(
        store,
        &send_key,
        "send token",
        "corky draft send",
        GMAIL_SEND_SCOPE,
        report,
    );

    // User info
    if !account.user.is_empty() {
        report.push(format!("  \u{2713} User: {}", account.user));
    }
}

fn check_imap_account(account: &crate::accounts::Account, report: &mut DoctorReport) {
    // Password configuration check
    if !account.password.is_empty() {
        report.push("  \u{2713} Password configured (inline)".to_string());
    } else if !account.password_cmd.is_empty() {
        report.push("  \u{2713} Password command configured".to_string());
    } else {
        report.fail("  \u{2717} No password or password_cmd configured".to_string());
    }

    // IMAP host info
    if !account.imap_host.is_empty() {
        report.push(format!(
            "  \u{2713} IMAP host: {}:{}",
            account.imap_host, account.imap_port
        ));
    } else {
        // Provider presets fill these in at load time, so empty means truly missing
        report.fail("  \u{2717} IMAP host not configured".to_string());
    }
}

fn check_token_status(
    store: &TokenStore,
    key: &str,
    label: &str,
    fix_cmd: &str,
    report: &mut DoctorReport,
) {
    check_token_status_with_scope(store, key, label, fix_cmd, "", report);
}

fn check_token_status_with_scope(
    store: &TokenStore,
    key: &str,
    label: &str,
    fix_cmd: &str,
    required_scope: &str,
    report: &mut DoctorReport,
) {
    let tp = tokens_path();
    match store.tokens.get(key) {
        Some(token) => {
            report.push(format!("  \u{2713} Token exists: {}", tp.display()));
            if !required_scope.is_empty() && !token_scopes_cover(&token.scopes, required_scope) {
                report.fail(format!(
                    "  \u{2717} Token scopes insufficient for {} ({})",
                    label, required_scope
                ));
            }
            if token.is_valid() {
                report.push(format!(
                    "  \u{2713} Token valid (expires {})",
                    token.expires_at.format("%Y-%m-%d %H:%M UTC")
                ));
            } else if token.refresh_token.is_some() {
                report.push(format!(
                    "  \u{2717} Token expired (expired {}) \u{2014} will auto-refresh on next `{}`",
                    token.expires_at.format("%Y-%m-%d %H:%M UTC"),
                    fix_cmd,
                ));
            } else {
                report.fail(format!(
                    "  \u{2717} Token expired with no refresh token \u{2014} run `{}`",
                    fix_cmd
                ));
            }
        }
        None => {
            report.fail(format!(
                "  \u{2717} No {} found (key: {}) \u{2014} run `{}`",
                label, key, fix_cmd
            ));
        }
    }
}

fn build_gmail_connector_status(cfg: &CorkyConfig, store: &TokenStore) -> GmailConnectorStatus {
    let (credentials_present, credential_source) = resolve_gmail_credential_source(cfg);
    let mut gmail_api_accounts: Vec<GmailAccountStatus> = cfg
        .accounts
        .iter()
        .filter(|(_, account)| account.provider == "gmail-api")
        .map(|(name, account)| GmailAccountStatus {
            account_name: name.clone(),
            user: account.user.clone(),
            sync_token: token_status(store, &format!("gmail:{}", account.user), GMAIL_SYNC_SCOPE),
            send_token: token_status(store, &format!("gmail:{}:send", name), GMAIL_SEND_SCOPE),
        })
        .collect();
    gmail_api_accounts.sort_by(|a, b| a.account_name.cmp(&b.account_name));

    GmailConnectorStatus {
        credentials_present,
        credential_source,
        tokens_path: tokens_path().display().to_string(),
        default_token: token_status(store, "gmail:default", GMAIL_FILTER_REQUIRED_SCOPE),
        gmail_api_accounts,
    }
}

fn resolve_gmail_credential_source(cfg: &CorkyConfig) -> (bool, String) {
    let has_config_creds = cfg.gmail.as_ref().is_some_and(|g| {
        !g.client_id.is_empty()
            || !g.client_id_cmd.is_empty()
            || !g.client_secret.is_empty()
            || !g.client_secret_cmd.is_empty()
    });
    if has_config_creds {
        return (true, "config".to_string());
    }

    let has_env_creds = std::env::var("CORKY_GMAIL_CLIENT_ID")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
        || std::env::var("CORKY_GMAIL_CLIENT_SECRET")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
    if has_env_creds {
        return (true, "env".to_string());
    }

    if !DEFAULT_GCP_CLIENT_ID.is_empty() {
        return (true, "built-in-defaults".to_string());
    }

    (false, "missing".to_string())
}

fn token_status(store: &TokenStore, key: &str, required_scope: &str) -> TokenStatus {
    let required_scopes = required_scope
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(token) = store.tokens.get(key) {
        build_token_status(key, token, required_scopes, required_scope)
    } else {
        TokenStatus {
            key: key.to_string(),
            present: false,
            valid: false,
            expires_at: None,
            has_refresh_token: false,
            scopes: vec![],
            required_scopes,
            scope_coverage: false,
            reauth_required: true,
        }
    }
}

fn build_token_status(
    key: &str,
    token: &StoredToken,
    required_scopes: Vec<String>,
    required_scope: &str,
) -> TokenStatus {
    let scope_coverage = token_scopes_cover(&token.scopes, required_scope);
    let valid = token.is_valid();
    let has_refresh_token = token.refresh_token.is_some();
    TokenStatus {
        key: key.to_string(),
        present: true,
        valid,
        expires_at: Some(token.expires_at.to_rfc3339()),
        has_refresh_token,
        scopes: token.scopes.clone(),
        required_scopes,
        scope_coverage,
        reauth_required: !scope_coverage || (!valid && !has_refresh_token),
    }
}

fn token_scopes_cover(token_scopes: &[String], required_scope: &str) -> bool {
    if required_scope.is_empty() {
        return true;
    }
    required_scope.split_whitespace().all(|req| {
        token_scopes.iter().any(|scope| scope == req)
            || (req.ends_with(".readonly")
                && token_scopes
                    .iter()
                    .any(|scope| scope == req.trim_end_matches(".readonly")))
    })
}
