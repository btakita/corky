//! Gmail OAuth2 authorization code flow for the Gmail Settings API.

use anyhow::{bail, Result};
use chrono::{Duration, Utc};

use crate::config::corky_config;
use crate::social::token_store::{StoredToken, TokenStore};

const REDIRECT_URI: &str = "http://127.0.0.1:8484/callback";
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
pub const DEFAULT_GCP_CLIENT_ID: &str = env!("CORKY_DEFAULT_GCP_CLIENT_ID");
pub const DEFAULT_GCP_CLIENT_SECRET: &str = env!("CORKY_DEFAULT_GCP_CLIENT_SECRET");

/// OAuth2 scopes for Gmail filter management.
/// - gmail.settings.basic: read/write filter settings
/// - gmail.labels: list labels (needed for name→ID resolution in push)
const GMAIL_FILTER_SCOPE: &str = "https://www.googleapis.com/auth/gmail.settings.basic https://www.googleapis.com/auth/gmail.labels";

/// OAuth2 scopes for Gmail API sync (read-only message access).
pub const GMAIL_SYNC_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";

/// Default scope (filter management) for backwards compatibility.
const GMAIL_SCOPE: &str = GMAIL_FILTER_SCOPE;

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
    let (cfg_id, cfg_id_cmd, cfg_secret, cfg_secret_cmd) =
        if let Some(cfg) = corky_config::try_load_config(None)
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

/// Get a valid access token, refreshing or running full auth flow if needed.
pub fn get_access_token(account: Option<&str>) -> Result<String> {
    get_access_token_with_scope(account, GMAIL_SCOPE)
}

/// Get a valid access token with specific scopes and optional login hint.
pub fn get_access_token_with_scope(account: Option<&str>, scope: &str) -> Result<String> {
    get_access_token_for_user(account, scope, None)
}

/// Get a valid access token with specific scopes and login hint (email).
pub fn get_access_token_for_user(account: Option<&str>, scope: &str, login_hint: Option<&str>) -> Result<String> {
    let key = token_key(account);
    let mut store = TokenStore::load()?;

    // Check for existing valid token
    if let Some(token) = store.get_valid(&key) {
        return Ok(token.access_token.clone());
    }

    // Try refresh if we have a refresh token
    if let Some(token) = store.tokens.get(&key).cloned()
        && let Some(ref refresh) = token.refresh_token {
            println!("Access token expired, refreshing...");
            match refresh_access_token(refresh) {
                Ok(new_token) => {
                    let access = new_token.access_token.clone();
                    store.upsert(key, new_token);
                    store.save()?;
                    return Ok(access);
                }
                Err(e) => {
                    eprintln!("Token refresh failed: {}. Re-authenticating...", e);
                }
            }
        }

    // Full auth flow with specified scope
    let token = run_auth_flow_with_scope(scope, login_hint)?;
    let access = token.access_token.clone();
    store.upsert(key, token);
    store.save()?;
    Ok(access)
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
            && let Ok(new_token) = refresh_access_token(refresh) {
                let access = new_token.access_token.clone();
                store.upsert(key, new_token);
                store.save()?;
                return Ok(access);
            }

    bail!("Gmail token expired or missing. Run `corky filter auth` to re-authenticate.")
}

/// Run explicit Gmail OAuth2 authentication (stores token).
pub fn run_auth(account: Option<&str>) -> Result<()> {
    let key = token_key(account);
    let token = run_auth_flow()?;
    let mut store = TokenStore::load()?;
    store.upsert(key.clone(), token);
    store.save()?;
    println!("Gmail token stored as '{}'", key);
    Ok(())
}

/// Run the full Gmail OAuth2 authorization code flow.
fn run_auth_flow() -> Result<StoredToken> {
    run_auth_flow_with_scope(GMAIL_SCOPE, None)
}

/// Run the full Gmail OAuth2 authorization code flow with specific scopes.
fn run_auth_flow_with_scope(scope: &str, login_hint: Option<&str>) -> Result<StoredToken> {
    let creds = resolve_credentials()?;
    let state = generate_state();

    let mut url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?response_type=code\
         &client_id={}\
         &redirect_uri={}\
         &state={}\
         &scope={}\
         &access_type=offline\
         &prompt=consent",
        urlencode(&creds.client_id),
        urlencode(REDIRECT_URI),
        urlencode(&state),
        urlencode(scope),
    );

    // Add login_hint to pre-select the correct Google account
    if let Some(hint) = login_hint {
        url.push_str(&format!("&login_hint={}", urlencode(hint)));
    }

    println!("Opening browser for Gmail authorization...");
    println!("If the browser doesn't open, visit:\n  {}\n", url);

    if open::that(&url).is_err() {
        eprintln!("Could not open browser automatically.");
    }

    // Start local callback server
    println!("Waiting for callback on {}...", REDIRECT_URI);
    let server = tiny_http::Server::http("127.0.0.1:8484")
        .map_err(|e| anyhow::anyhow!("Failed to start callback server: {}", e))?;

    let request = server
        .recv_timeout(std::time::Duration::from_secs(CALLBACK_TIMEOUT_SECS))
        .map_err(|e| anyhow::anyhow!("Callback server error: {}", e))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Timed out waiting for OAuth callback ({}s)",
                CALLBACK_TIMEOUT_SECS
            )
        })?;

    // Parse callback
    let url_str = request.url().to_string();
    let query = url_str.split('?').nth(1).unwrap_or("");
    let (code, cb_state) = crate::social::auth::parse_callback(query)?;

    // Respond to the browser
    let response =
        tiny_http::Response::from_string("Gmail authorization successful! You can close this tab.");
    let _ = request.respond(response);

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
    exchange_code(&creds, &code, scope)
}

/// Exchange an authorization code for access + refresh tokens.
fn exchange_code(creds: &ClientCredentials, code: &str, scope: &str) -> Result<StoredToken> {
    let body_str = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&client_secret={}",
        urlencode(code),
        urlencode(REDIRECT_URI),
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
fn refresh_access_token(refresh_token: &str) -> Result<StoredToken> {
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
    let mut token = parse_token_response(&body, GMAIL_SCOPE)?;
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

    Ok(StoredToken {
        access_token,
        refresh_token,
        expires_at: Utc::now() + Duration::seconds(expires_in),
        scopes: vec![scope.to_string()],
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
        assert!(token.scopes[0].contains("gmail.settings.basic"));
        assert!(token.is_valid());
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
    fn test_parse_token_response_missing_access_token() {
        let body = serde_json::json!({
            "expires_in": 3600,
            "token_type": "Bearer"
        });
        assert!(parse_token_response(&body, GMAIL_SCOPE).is_err());
    }
}
