//! Gmail OAuth2 authorization code flow for the Gmail Settings API.

use anyhow::{Result, bail};
use chrono::{Duration, Utc};

use crate::config::corky_config;
use crate::desktop_notify::notify_oauth;
use crate::oauth_loopback::{LoopbackServer, PortMode};
use crate::social::token_store::{StoredToken, TokenStore};

const CALLBACK_TIMEOUT_SECS: u64 = 300;

/// Default GCP OAuth2 client credentials for the corky desktop application.
///
/// These are **public** credentials for a "Desktop app" OAuth client, which Google
/// explicitly documents as non-secret:
/// <https://developers.google.com/identity/protocols/oauth2#installed-applications>
///
/// Injected at build time via environment variables. Set `CORKY_DEFAULT_GCP_CLIENT_ID`
/// and `CORKY_DEFAULT_GCP_CLIENT_SECRET` when building, or use the values from
/// `pass corky/gcp/client_id` and `pass corky/gcp/client_secret`.
///
/// Users can override these via `[gmail]` in `.corky.toml` or env vars.
pub const DEFAULT_GCP_CLIENT_ID: &str = match option_env!("CORKY_DEFAULT_GCP_CLIENT_ID") {
    Some(v) => v,
    None => "",
};
pub const DEFAULT_GCP_CLIENT_SECRET: &str = match option_env!("CORKY_DEFAULT_GCP_CLIENT_SECRET") {
    Some(v) => v,
    None => "",
};

/// OAuth2 scopes for Gmail filter management.
/// - gmail.settings.basic: read/write filter settings
/// - gmail.labels: list labels (needed for name→ID resolution in push)
pub const GMAIL_FILTER_SCOPE: &str = "https://www.googleapis.com/auth/gmail.settings.basic https://www.googleapis.com/auth/gmail.labels";

/// OAuth2 scopes for Gmail API sync (read-only message access).
pub const GMAIL_SYNC_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";

/// OAuth2 scope for Gmail API sending and draft creation.
/// `gmail.compose` covers both `messages.send` and `drafts.create`.
pub const GMAIL_SEND_SCOPE: &str = "https://www.googleapis.com/auth/gmail.compose";

/// OAuth2 scope for Google Drive file upload (restricted to files created by this app).
pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// OAuth2 scope for Google Drive read/export/download metadata access.
pub const DRIVE_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";

/// OAuth2 scope for Google Docs read/write.
pub const DOCS_SCOPE: &str = "https://www.googleapis.com/auth/documents";

/// OAuth2 scope for Google Sheets read-only.
pub const SHEETS_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets.readonly";

/// OAuth2 scope for Google Sheets read/write.
pub const SHEETS_SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets";

/// One-shot OAuth2 scope bundle for broad Google Workspace document workflows.
///
/// This lets users pre-authorize Drive upload, Drive metadata/export/download,
/// Docs read/write, and Sheets read/write with one browser consent flow instead
/// of discovering each document scope one command at a time.
pub const GOOGLE_WORKSPACE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file https://www.googleapis.com/auth/drive.readonly https://www.googleapis.com/auth/documents https://www.googleapis.com/auth/spreadsheets";

/// OAuth2 scope for Google Chat message sending.
pub const CHAT_SCOPE: &str = "https://www.googleapis.com/auth/chat.messages";

/// OAuth2 scope for Google Tasks read/write.
pub const TASKS_SCOPE: &str = "https://www.googleapis.com/auth/tasks";

/// Default scope (filter management) for backwards compatibility.
const GMAIL_SCOPE: &str = GMAIL_FILTER_SCOPE;

/// Check whether a token's stored scopes cover the requested scope.
///
/// A read-write scope (e.g. `spreadsheets`) subsumes its `.readonly` variant.
fn scope_covered(token_scopes: &[String], requested: &str) -> bool {
    let token_scopes: Vec<&str> = token_scopes
        .iter()
        .flat_map(|scope| scope.split_whitespace())
        .collect();
    for req in requested.split_whitespace() {
        let covered = token_scopes.contains(&req)
            || (req.ends_with(".readonly")
                && token_scopes.contains(&req.trim_end_matches(".readonly")));
        if !covered {
            return false;
        }
    }
    true
}

/// Client credentials resolved from .corky.toml or env vars.
struct ClientCredentials {
    client_id: String,
    client_secret: String,
}

/// Resolve Gmail OAuth2 client credentials.
///
/// Resolution order per field:
/// 1. Inline value in `[gmail]` section of .corky.toml
/// 2. Command (`client_id_cmd` / `client_secret_cmd`) in .corky.toml
/// 3. Environment variable (`CORKY_GMAIL_CLIENT_ID` / `CORKY_GMAIL_CLIENT_SECRET`)
/// 4. Built-in default (corky's public GCP desktop-app credentials)
fn resolve_credentials() -> Result<ClientCredentials> {
    let (cfg_id, cfg_id_cmd, cfg_secret, cfg_secret_cmd) = if let Some(cfg) =
        corky_config::try_load_config(None)
        && let Some(gmail) = &cfg.gmail
    {
        (
            gmail.client_id.clone(),
            gmail.client_id_cmd.clone(),
            gmail.client_secret.clone(),
            gmail.client_secret_cmd.clone(),
        )
    } else {
        Default::default()
    };

    let client_id = resolve_credential_field(
        &cfg_id,
        &cfg_id_cmd,
        "CORKY_GMAIL_CLIENT_ID",
        DEFAULT_GCP_CLIENT_ID,
    )?;
    let client_secret = resolve_credential_field(
        &cfg_secret,
        &cfg_secret_cmd,
        "CORKY_GMAIL_CLIENT_SECRET",
        DEFAULT_GCP_CLIENT_SECRET,
    )?;

    Ok(ClientCredentials {
        client_id,
        client_secret,
    })
}

/// Resolve a single credential field through the fallback chain:
/// inline value → command → env var → built-in default.
fn resolve_credential_field(
    inline: &str,
    cmd: &str,
    env_var: &str,
    default: &str,
) -> Result<String> {
    // 1. Inline config value
    if !inline.is_empty() {
        return Ok(inline.to_string());
    }
    // 2. Command
    if !cmd.is_empty() {
        return crate::util::resolve_secret("", cmd, env_var);
    }
    // 3. Environment variable
    if let Ok(val) = std::env::var(env_var)
        && !val.is_empty()
    {
        return Ok(val);
    }
    // 4. Built-in default
    Ok(default.to_string())
}

/// Percent-encode a string for URL query parameters / form bodies.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

/// Generate a random state parameter for CSRF protection.
fn generate_state() -> String {
    use std::time::SystemTime;
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}", nonce)
}

/// Token store key for a Gmail account.
fn token_key(account: Option<&str>) -> String {
    match account {
        Some(name) => format!("gmail:{}", name),
        None => "gmail:default".to_string(),
    }
}

fn token_key_for_user(account: Option<&str>, login_hint: Option<&str>) -> String {
    if let Some(hint) = login_hint {
        format!("gmail:{}", hint)
    } else {
        token_key(account)
    }
}

/// Token store key for a Gmail account with a scope suffix.
/// Used to avoid collisions between tokens with different scopes (e.g., sync vs send).
fn scoped_token_key(account: Option<&str>, scope_suffix: &str) -> String {
    let base = token_key(account);
    format!("{}:{}", base, scope_suffix)
}

/// Get a valid access token, refreshing or running full auth flow if needed.
pub fn get_access_token(account: Option<&str>) -> Result<String> {
    get_access_token_with_scope(account, GMAIL_SCOPE)
}

/// Get a valid access token with specific scopes and optional login hint.
pub fn get_access_token_with_scope(account: Option<&str>, scope: &str) -> Result<String> {
    get_access_token_for_user(account, scope, None)
}

/// Get a valid access token with specific scopes and login hint (email).
///
/// When `login_hint` is provided, the token is stored under a key derived from
/// the email (e.g., `gmail:brian.takita@gmail.com`) so different Google accounts
/// get separate cached tokens.
pub fn get_access_token_for_user(
    account: Option<&str>,
    scope: &str,
    login_hint: Option<&str>,
) -> Result<String> {
    let key = token_key_for_user(account, login_hint);
    let mut store = TokenStore::load()?;

    // Check for existing valid token with sufficient scope
    if let Some(token) = store.get_valid(&key) {
        if scope_covered(&token.scopes, scope) {
            return Ok(token.access_token.clone());
        }
        eprintln!("Cached token has insufficient scope for {scope}. Re-authenticating...");
    }

    // Try refresh if we have a refresh token AND stored scopes are sufficient
    // (refresh preserves original scopes, so refreshing won't help if scopes are wrong)
    let mut refresh_failed = false;
    if let Some(token) = store.tokens.get(&key).cloned()
        && let Some(ref refresh) = token.refresh_token
        && scope_covered(&token.scopes, scope)
    {
        println!("Access token expired, refreshing...");
        match refresh_access_token(refresh, &token.scopes) {
            Ok(new_token) => {
                let access = new_token.access_token.clone();
                store.upsert(key, new_token);
                store.save()?;
                return Ok(access);
            }
            Err(e) => {
                eprintln!("Token refresh failed: {}. Re-authenticating...", e);
                refresh_failed = true;
            }
        }
    }

    // Full/incremental auth flow with specified scope. If Google omits a new
    // refresh token during incremental auth, keep the refresh grant we already
    // have instead of downgrading the cache to access-token-only.
    let previous = store.tokens.get(&key).cloned();
    let token = run_auth_flow_with_scope(
        scope,
        login_hint,
        prompt_consent_for(previous.as_ref(), refresh_failed),
    )?;
    let token = merge_cached_grant(token, previous.as_ref());
    ensure_scope_covered(&token.scopes, scope)?;
    let access = token.access_token.clone();
    store.upsert(key, token);
    store.save()?;
    Ok(access)
}

/// Get a valid access token for Gmail API sending.
///
/// Uses a dedicated token key (`gmail:<account>:send`) to avoid collision
/// with the sync token (which has `gmail.readonly` scope).
pub fn get_send_access_token(account: Option<&str>, login_hint: Option<&str>) -> Result<String> {
    let key = scoped_token_key(account, "send");
    let mut store = TokenStore::load()?;

    // Check for existing valid token
    if let Some(token) = store.get_valid(&key) {
        if scope_covered(&token.scopes, GMAIL_SEND_SCOPE) {
            return Ok(token.access_token.clone());
        }
        eprintln!("Cached send token has insufficient scope. Re-authenticating...");
    }

    // Try refresh if we have a refresh token
    let mut refresh_failed = false;
    if let Some(token) = store.tokens.get(&key).cloned()
        && let Some(ref refresh) = token.refresh_token
        && scope_covered(&token.scopes, GMAIL_SEND_SCOPE)
    {
        println!("Send token expired, refreshing...");
        match refresh_access_token(refresh, &token.scopes) {
            Ok(new_token) => {
                let access = new_token.access_token.clone();
                store.upsert(key, new_token);
                store.save()?;
                return Ok(access);
            }
            Err(e) => {
                eprintln!("Token refresh failed: {}. Re-authenticating...", e);
                refresh_failed = true;
            }
        }
    }

    // Full/incremental auth flow with send scope.
    let previous = store.tokens.get(&key).cloned();
    let token = run_auth_flow_with_scope(
        GMAIL_SEND_SCOPE,
        login_hint,
        prompt_consent_for(previous.as_ref(), refresh_failed),
    )?;
    let token = merge_cached_grant(token, previous.as_ref());
    ensure_scope_covered(&token.scopes, GMAIL_SEND_SCOPE)?;
    let access = token.access_token.clone();
    store.upsert(key, token);
    store.save()?;
    Ok(access)
}

/// Remove the cached Gmail send token so the next send flow re-authenticates.
pub fn clear_send_token(account: Option<&str>) -> Result<bool> {
    let key = scoped_token_key(account, "send");
    let mut store = TokenStore::load()?;
    store.remove_persisted(&key)
}

/// Get a valid access token without interactive auth.
///
/// Returns the cached/refreshed token if available, or an error with
/// an actionable message telling the user to run `corky filter auth`.
/// Used by watch mode to avoid opening a browser unexpectedly.
pub fn get_access_token_noninteractive(account: Option<&str>) -> Result<String> {
    let key = token_key(account);
    let mut store = TokenStore::load()?;

    if let Some(token) = store.get_valid(&key) {
        return Ok(token.access_token.clone());
    }

    if let Some(token) = store.tokens.get(&key).cloned()
        && let Some(ref refresh) = token.refresh_token
        && let Ok(new_token) = refresh_access_token(refresh, &token.scopes)
    {
        let access = new_token.access_token.clone();
        store.upsert(key, new_token);
        store.save()?;
        return Ok(access);
    }

    bail!("Gmail token expired or missing. Run `corky filter auth` to re-authenticate.")
}

/// Run explicit Gmail OAuth2 authentication (stores token).
pub fn run_auth(account: Option<&str>) -> Result<()> {
    let login_hint = account.filter(|value| value.contains('@'));
    run_auth_with_scope(account, GMAIL_SCOPE, login_hint)
}

/// Run explicit Gmail OAuth2 authentication for a requested scope.
///
/// When `login_hint` is provided, the token is stored under `gmail:<email>`
/// so manual auth matches the automatic Gmail API sync/docs/sheets lookup path.
pub fn run_auth_with_scope(
    account: Option<&str>,
    scope: &str,
    login_hint: Option<&str>,
) -> Result<()> {
    let key = token_key_for_user(account, login_hint);
    let mut store = TokenStore::load()?;
    let previous = store.tokens.get(&key).cloned();
    let token = run_auth_flow_with_scope(
        scope,
        login_hint,
        prompt_consent_for(previous.as_ref(), true),
    )?;
    let token = merge_cached_grant(token, previous.as_ref());
    ensure_scope_covered(&token.scopes, scope)?;
    store.upsert(key.clone(), token);
    store.save()?;
    println!("Gmail token stored as '{}'", key);
    Ok(())
}

/// Run explicit Gmail compose authentication.
///
/// Compose tokens stay under `gmail:<account>:send` because draft push/send
/// resolves by configured account name while using `login_hint` only for the
/// browser account picker.
pub fn run_send_auth(account: Option<&str>, login_hint: Option<&str>) -> Result<()> {
    let key = scoped_token_key(account, "send");
    let mut store = TokenStore::load()?;
    let previous = store.tokens.get(&key).cloned();
    let token = run_auth_flow_with_scope(
        GMAIL_SEND_SCOPE,
        login_hint,
        prompt_consent_for(previous.as_ref(), true),
    )?;
    let token = merge_cached_grant(token, previous.as_ref());
    ensure_scope_covered(&token.scopes, GMAIL_SEND_SCOPE)?;
    store.upsert(key.clone(), token);
    store.save()?;
    println!("Gmail send token stored as '{}'", key);
    Ok(())
}

/// Run the full Gmail OAuth2 authorization code flow with specific scopes.
fn run_auth_flow_with_scope(
    scope: &str,
    login_hint: Option<&str>,
    prompt_consent: bool,
) -> Result<StoredToken> {
    let creds = resolve_credentials()?;
    let state = generate_state();
    let callback = LoopbackServer::bind("Gmail", PortMode::OptInEphemeralFallback)?;
    let redirect_uri = callback.redirect_uri().to_string();
    let url = build_auth_url(
        &creds.client_id,
        &redirect_uri,
        &state,
        scope,
        login_hint,
        prompt_consent,
    );

    notify_oauth("Gmail");
    println!("Opening browser for Gmail authorization...");
    println!("If the browser doesn't open, visit:\n  {}\n", url);

    if open::that(&url).is_err() {
        eprintln!("Could not open browser automatically.");
    }

    println!("Waiting for callback on {}...", redirect_uri);
    let callback = callback.recv_callback(CALLBACK_TIMEOUT_SECS)?;
    let code = callback.code.clone();
    let cb_state = callback.state.clone();
    callback.respond_text("Gmail authorization successful! You can close this tab.");

    // Verify state (CSRF protection)
    if cb_state != state {
        bail!(
            "State mismatch (CSRF). Expected '{}', got '{}'",
            state,
            cb_state
        );
    }

    // Exchange code for token
    println!("Exchanging authorization code...");
    exchange_code(&creds, &code, &redirect_uri, scope)
}

fn build_auth_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scope: &str,
    login_hint: Option<&str>,
    prompt_consent: bool,
) -> String {
    let mut url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?response_type=code\
         &client_id={}\
         &redirect_uri={}\
         &state={}\
         &scope={}\
         &access_type=offline\
         &include_granted_scopes=true",
        urlencode(client_id),
        urlencode(redirect_uri),
        urlencode(state),
        urlencode(scope),
    );

    if prompt_consent {
        url.push_str("&prompt=consent");
    }

    // Add login_hint to pre-select the correct Google account
    if let Some(hint) = login_hint {
        url.push_str(&format!("&login_hint={}", urlencode(hint)));
    }

    url
}

fn prompt_consent_for(previous: Option<&StoredToken>, force_consent: bool) -> bool {
    if force_consent {
        return true;
    }
    previous
        .and_then(|token| token.refresh_token.as_ref())
        .is_none()
}

fn merge_cached_grant(mut token: StoredToken, previous: Option<&StoredToken>) -> StoredToken {
    if let Some(previous) = previous {
        if token.refresh_token.is_none() {
            token.refresh_token = previous.refresh_token.clone();
        }
        if previous.refresh_token.is_some() {
            token.scopes = merge_scopes(&token.scopes, &previous.scopes);
        }
    }
    token
}

fn merge_scopes(primary: &[String], secondary: &[String]) -> Vec<String> {
    let mut scopes = Vec::new();
    for scope in primary.iter().chain(secondary) {
        for part in scope.split_whitespace() {
            if !scopes.iter().any(|existing| existing == part) {
                scopes.push(part.to_string());
            }
        }
    }
    scopes
}

fn ensure_scope_covered(token_scopes: &[String], requested: &str) -> Result<()> {
    if scope_covered(token_scopes, requested) {
        return Ok(());
    }

    let granted = if token_scopes.is_empty() {
        "(none)".to_string()
    } else {
        token_scopes.join(" ")
    };
    bail!(
        "Google OAuth token is missing required scope(s): {requested}. Granted scope(s): {granted}. Re-run the command and approve the requested Google permissions, or run `corky auth --scope workspace` for document workflows."
    )
}

/// Exchange an authorization code for access + refresh tokens.
fn exchange_code(
    creds: &ClientCredentials,
    code: &str,
    redirect_uri: &str,
    scope: &str,
) -> Result<StoredToken> {
    let body_str = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&client_secret={}",
        urlencode(code),
        urlencode(redirect_uri),
        urlencode(&creds.client_id),
        urlencode(&creds.client_secret),
    );

    let resp = match ureq::post("https://oauth2.googleapis.com/token")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body_str)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Token exchange failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => return Err(e.into()),
    };

    let body: serde_json::Value = resp.into_json()?;
    parse_token_response(&body, scope)
}

/// Refresh an expired access token using the refresh token.
fn refresh_access_token(refresh_token: &str, existing_scopes: &[String]) -> Result<StoredToken> {
    let creds = resolve_credentials()?;
    let body_str = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
        urlencode(refresh_token),
        urlencode(&creds.client_id),
        urlencode(&creds.client_secret),
    );

    let resp = match ureq::post("https://oauth2.googleapis.com/token")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body_str)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Token refresh failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => return Err(e.into()),
    };

    let body: serde_json::Value = resp.into_json()?;
    parse_refresh_token_response(&body, refresh_token, existing_scopes)
}

fn parse_refresh_token_response(
    body: &serde_json::Value,
    refresh_token: &str,
    existing_scopes: &[String],
) -> Result<StoredToken> {
    let fallback_scope = existing_scopes.join(" ");
    let mut token = parse_token_response(body, &fallback_scope)?;
    // Refresh responses don't include a new refresh_token — keep the original
    token.refresh_token = Some(refresh_token.to_string());
    Ok(token)
}

/// Parse a Google OAuth2 token response into a StoredToken.
fn parse_token_response(body: &serde_json::Value, scope: &str) -> Result<StoredToken> {
    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing access_token in response"))?
        .to_string();
    let expires_in = body["expires_in"].as_i64().unwrap_or(3600);
    let refresh_token = body["refresh_token"].as_str().map(|s| s.to_string());
    let scope_str = body["scope"].as_str().unwrap_or(scope);
    let scopes = scope_str
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();

    Ok(StoredToken {
        access_token,
        refresh_token,
        expires_at: Utc::now() + Duration::seconds(expires_in),
        scopes,
        platform: "gmail".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencode_basic() {
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("a@b.com"), "a%40b.com");
    }

    #[test]
    fn test_urlencode_preserves_unreserved() {
        assert_eq!(urlencode("abc-._~123"), "abc-._~123");
    }

    #[test]
    fn test_token_key_default() {
        assert_eq!(token_key(None), "gmail:default");
    }

    #[test]
    fn test_token_key_named() {
        assert_eq!(token_key(Some("work")), "gmail:work");
    }

    #[test]
    fn test_token_key_for_user_prefers_login_hint() {
        assert_eq!(
            token_key_for_user(Some("work"), Some("person@example.com")),
            "gmail:person@example.com"
        );
    }

    #[test]
    fn test_parse_token_response() {
        let body = serde_json::json!({
            "access_token": "ya29.test",
            "expires_in": 3600,
            "refresh_token": "1//test",
            "token_type": "Bearer"
        });
        let token = parse_token_response(&body, GMAIL_SCOPE).unwrap();
        assert_eq!(token.access_token, "ya29.test");
        assert_eq!(token.refresh_token.as_deref(), Some("1//test"));
        assert_eq!(token.platform, "gmail");
        assert!(
            token
                .scopes
                .contains(&"https://www.googleapis.com/auth/gmail.settings.basic".to_string())
        );
        assert!(
            token
                .scopes
                .contains(&"https://www.googleapis.com/auth/gmail.labels".to_string())
        );
        assert!(token.is_valid());
    }

    #[test]
    fn test_parse_token_response_uses_returned_scope() {
        let body = serde_json::json!({
            "access_token": "ya29.test",
            "expires_in": 3600,
            "scope": GMAIL_SYNC_SCOPE,
            "token_type": "Bearer"
        });
        let token = parse_token_response(&body, GMAIL_FILTER_SCOPE).unwrap();
        assert_eq!(token.scopes, vec![GMAIL_SYNC_SCOPE.to_string()]);
    }

    #[test]
    fn test_parse_token_response_no_refresh() {
        let body = serde_json::json!({
            "access_token": "ya29.test",
            "expires_in": 3600,
            "token_type": "Bearer"
        });
        let token = parse_token_response(&body, GMAIL_SCOPE).unwrap();
        assert!(token.refresh_token.is_none());
    }

    #[test]
    fn test_scope_covered_exact() {
        let scopes = vec![SHEETS_SCOPE.to_string()];
        assert!(scope_covered(&scopes, SHEETS_SCOPE));
        assert!(!scope_covered(&scopes, GMAIL_SYNC_SCOPE));
    }

    #[test]
    fn test_scope_covered_rw_subsumes_readonly() {
        let scopes = vec![SHEETS_SCOPE.to_string()];
        assert!(scope_covered(&scopes, SHEETS_READONLY_SCOPE));
    }

    #[test]
    fn test_scope_covered_readonly_does_not_subsume_rw() {
        let scopes = vec![SHEETS_READONLY_SCOPE.to_string()];
        assert!(!scope_covered(&scopes, SHEETS_SCOPE));
    }

    #[test]
    fn test_ensure_scope_covered_rejects_readonly_for_sheets_write() {
        let scopes = vec![SHEETS_READONLY_SCOPE.to_string()];
        let err = ensure_scope_covered(&scopes, SHEETS_SCOPE).unwrap_err();
        let message = err.to_string();

        assert!(message.contains(SHEETS_SCOPE));
        assert!(message.contains("corky auth --scope workspace"));
    }

    #[test]
    fn test_scope_covered_multi_scope_string() {
        let scopes = vec![
            "https://www.googleapis.com/auth/gmail.settings.basic".to_string(),
            "https://www.googleapis.com/auth/gmail.labels".to_string(),
        ];
        assert!(scope_covered(
            &scopes,
            "https://www.googleapis.com/auth/gmail.settings.basic https://www.googleapis.com/auth/gmail.labels"
        ));
        assert!(!scope_covered(
            &scopes,
            "https://www.googleapis.com/auth/gmail.settings.basic https://www.googleapis.com/auth/gmail.compose"
        ));
    }

    #[test]
    fn test_scope_covered_token_scope_with_spaces() {
        let scopes = vec![GMAIL_FILTER_SCOPE.to_string()];
        assert!(scope_covered(&scopes, GMAIL_FILTER_SCOPE));
    }

    #[test]
    fn test_workspace_scope_covers_document_workflows() {
        let scopes = vec![GOOGLE_WORKSPACE_SCOPE.to_string()];

        assert!(scope_covered(&scopes, DRIVE_FILE_SCOPE));
        assert!(scope_covered(&scopes, DRIVE_READONLY_SCOPE));
        assert!(scope_covered(&scopes, DOCS_SCOPE));
        assert!(scope_covered(&scopes, SHEETS_SCOPE));
        assert!(scope_covered(&scopes, SHEETS_READONLY_SCOPE));
    }

    #[test]
    fn test_build_auth_url_uses_incremental_auth_without_prompt_by_default() {
        let url = build_auth_url(
            "client id",
            "http://127.0.0.1:8484/callback",
            "state",
            SHEETS_SCOPE,
            Some("person@example.com"),
            false,
        );

        assert!(url.contains("include_granted_scopes=true"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("login_hint=person%40example.com"));
        assert!(!url.contains("prompt=consent"));
    }

    #[test]
    fn test_build_auth_url_can_force_consent_when_refresh_grant_is_missing() {
        let url = build_auth_url(
            "client",
            "http://127.0.0.1:8484/callback",
            "state",
            SHEETS_SCOPE,
            None,
            true,
        );

        assert!(url.contains("prompt=consent"));
    }

    #[test]
    fn test_prompt_consent_for_existing_refresh_token() {
        let token = StoredToken {
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Utc::now() + Duration::hours(1),
            scopes: vec![SHEETS_SCOPE.to_string()],
            platform: "gmail".to_string(),
        };

        assert!(!prompt_consent_for(Some(&token), false));
        assert!(prompt_consent_for(Some(&token), true));
        assert!(prompt_consent_for(None, false));
    }

    #[test]
    fn test_merge_cached_grant_preserves_refresh_token_and_scope_union() {
        let previous = StoredToken {
            access_token: "old-access".to_string(),
            refresh_token: Some("old-refresh".to_string()),
            expires_at: Utc::now() + Duration::hours(1),
            scopes: vec![GMAIL_FILTER_SCOPE.to_string()],
            platform: "gmail".to_string(),
        };
        let new_token = StoredToken {
            access_token: "new-access".to_string(),
            refresh_token: None,
            expires_at: Utc::now() + Duration::hours(1),
            scopes: vec![SHEETS_SCOPE.to_string()],
            platform: "gmail".to_string(),
        };

        let merged = merge_cached_grant(new_token, Some(&previous));

        assert_eq!(merged.access_token, "new-access");
        assert_eq!(merged.refresh_token.as_deref(), Some("old-refresh"));
        assert!(scope_covered(&merged.scopes, GMAIL_FILTER_SCOPE));
        assert!(scope_covered(&merged.scopes, SHEETS_SCOPE));
    }

    #[test]
    fn test_merge_cached_grant_without_refresh_uses_returned_scopes_only() {
        let previous = StoredToken {
            access_token: "old-access".to_string(),
            refresh_token: None,
            expires_at: Utc::now() + Duration::hours(1),
            scopes: vec![GMAIL_FILTER_SCOPE.to_string()],
            platform: "gmail".to_string(),
        };
        let new_token = StoredToken {
            access_token: "new-access".to_string(),
            refresh_token: Some("new-refresh".to_string()),
            expires_at: Utc::now() + Duration::hours(1),
            scopes: vec![SHEETS_SCOPE.to_string()],
            platform: "gmail".to_string(),
        };

        let merged = merge_cached_grant(new_token, Some(&previous));

        assert_eq!(merged.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(merged.scopes, vec![SHEETS_SCOPE.to_string()]);
    }

    #[test]
    fn test_parse_refresh_token_response_preserves_existing_scopes() {
        let body = serde_json::json!({
            "access_token": "ya29.refreshed",
            "expires_in": 3600,
            "token_type": "Bearer"
        });
        let existing_scopes = vec![GMAIL_SYNC_SCOPE.to_string()];
        let token = parse_refresh_token_response(&body, "refresh-token", &existing_scopes).unwrap();
        assert_eq!(token.access_token, "ya29.refreshed");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-token"));
        assert_eq!(token.scopes, existing_scopes);
    }

    #[test]
    fn test_parse_token_response_missing_access_token() {
        let body = serde_json::json!({
            "expires_in": 3600,
            "token_type": "Bearer"
        });
        assert!(parse_token_response(&body, GMAIL_SCOPE).is_err());
    }
}
