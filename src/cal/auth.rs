//! Google Calendar OAuth2 authorization code flow.
//!
//! Reuses the same Google OAuth2 client credentials as Gmail ([gmail] in .corky.toml)
//! but requests the Calendar scope. Tokens stored under "calendar:*" keys.

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};

use crate::config::corky_config;
use crate::desktop_notify::notify_oauth;
use crate::oauth_loopback::{LoopbackServer, PortMode};
use crate::social::token_store::{StoredToken, TokenStore};

const CALLBACK_TIMEOUT_SECS: u64 = 120;

/// OAuth2 scope for full calendar access.
const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar";

struct ClientCredentials {
    client_id: String,
    client_secret: String,
}

/// Resolve OAuth2 client credentials from .corky.toml [gmail] section or env vars.
/// Calendar reuses the same Google project credentials as Gmail.
fn resolve_credentials() -> Result<ClientCredentials> {
    if let Some(cfg) = corky_config::try_load_config(None)
        && let Some(gmail) = &cfg.gmail
    {
        let has_config = !gmail.client_id.is_empty()
            || !gmail.client_id_cmd.is_empty()
            || !gmail.client_secret.is_empty()
            || !gmail.client_secret_cmd.is_empty();
        if has_config {
            let client_id = crate::util::resolve_secret(
                &gmail.client_id,
                &gmail.client_id_cmd,
                "Gmail client_id (check [gmail] in .corky.toml)",
            )?;
            let client_secret = crate::util::resolve_secret(
                &gmail.client_secret,
                &gmail.client_secret_cmd,
                "Gmail client_secret (check [gmail] in .corky.toml)",
            )?;
            return Ok(ClientCredentials {
                client_id,
                client_secret,
            });
        }
    }
    let client_id = std::env::var("CORKY_GMAIL_CLIENT_ID").context(
        "Gmail client_id not found.\nSet [gmail] in .corky.toml or CORKY_GMAIL_CLIENT_ID env var.",
    )?;
    let client_secret = std::env::var("CORKY_GMAIL_CLIENT_SECRET")
        .context("Gmail client_secret not found.\nSet [gmail] in .corky.toml or CORKY_GMAIL_CLIENT_SECRET env var.")?;
    Ok(ClientCredentials {
        client_id,
        client_secret,
    })
}

/// Percent-encode a string for URL query parameters.
pub(crate) fn urlencode_pub(s: &str) -> String {
    urlencode(s)
}

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

fn generate_state() -> String {
    use std::time::SystemTime;
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}", nonce)
}

fn token_key(account: Option<&str>) -> String {
    match account {
        Some(name) => format!("calendar:{}", name),
        None => "calendar:default".to_string(),
    }
}

/// Get a valid Calendar access token, refreshing or running full auth flow if needed.
pub fn get_access_token(account: Option<&str>) -> Result<String> {
    let key = token_key(account);
    let mut store = TokenStore::load()?;

    if let Some(token) = store.get_valid(&key) {
        return Ok(token.access_token.clone());
    }

    // Try refresh
    if let Some(token) = store.tokens.get(&key).cloned()
        && let Some(ref refresh) = token.refresh_token
    {
        println!("Calendar token expired, refreshing...");
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

    let token = run_auth_flow()?;
    let access = token.access_token.clone();
    store.upsert(key, token);
    store.save()?;
    Ok(access)
}

/// Run explicit Calendar OAuth2 authentication.
pub fn run_auth(account: Option<&str>) -> Result<()> {
    let key = token_key(account);
    let token = run_auth_flow()?;
    let mut store = TokenStore::load()?;
    store.upsert(key.clone(), token);
    store.save()?;
    println!("Calendar token stored as '{}'", key);
    Ok(())
}

fn run_auth_flow() -> Result<StoredToken> {
    let creds = resolve_credentials()?;
    let state = generate_state();
    let callback = LoopbackServer::bind("Google Calendar", PortMode::EphemeralFallback)?;
    let redirect_uri = callback.redirect_uri().to_string();

    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?response_type=code\
         &client_id={}\
         &redirect_uri={}\
         &state={}\
         &scope={}\
         &access_type=offline\
         &prompt=consent",
        urlencode(&creds.client_id),
        urlencode(&redirect_uri),
        urlencode(&state),
        urlencode(CALENDAR_SCOPE),
    );

    notify_oauth("Google Calendar");
    println!("Opening browser for Google Calendar authorization...");
    println!("If the browser doesn't open, visit:\n  {}\n", url);

    if open::that(&url).is_err() {
        eprintln!("Could not open browser automatically.");
    }

    println!("Waiting for callback on {}...", redirect_uri);
    let callback = callback.recv_callback(CALLBACK_TIMEOUT_SECS)?;
    let code = callback.code.clone();
    let cb_state = callback.state.clone();
    callback.respond_text("Google Calendar authorization successful! You can close this tab.");

    if cb_state != state {
        bail!(
            "State mismatch (CSRF). Expected '{}', got '{}'",
            state,
            cb_state
        );
    }

    println!("Exchanging authorization code...");
    exchange_code(&creds, &code, &redirect_uri)
}

fn exchange_code(creds: &ClientCredentials, code: &str, redirect_uri: &str) -> Result<StoredToken> {
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
    parse_token_response(&body)
}

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
    let mut token = parse_token_response(&body)?;
    token.refresh_token = Some(refresh_token.to_string());
    Ok(token)
}

fn parse_token_response(body: &serde_json::Value) -> Result<StoredToken> {
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
        scopes: vec![CALENDAR_SCOPE.to_string()],
        platform: "calendar".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_key_default() {
        assert_eq!(token_key(None), "calendar:default");
    }

    #[test]
    fn test_token_key_named() {
        assert_eq!(token_key(Some("work")), "calendar:work");
    }

    #[test]
    fn test_parse_token_response() {
        let body = serde_json::json!({
            "access_token": "ya29.test",
            "expires_in": 3600,
            "refresh_token": "1//test",
            "token_type": "Bearer"
        });
        let token = parse_token_response(&body).unwrap();
        assert_eq!(token.access_token, "ya29.test");
        assert_eq!(token.refresh_token.as_deref(), Some("1//test"));
        assert_eq!(token.platform, "calendar");
        assert!(token.scopes[0].contains("calendar"));
    }
}
