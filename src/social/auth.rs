//! OAuth2 authorization code flow with PKCE for social platforms.

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};

use super::platform::Platform;
use super::token_store::{StoredToken, TokenStore};
use crate::config::corky_config;
use crate::desktop_notify::notify_oauth;
use crate::oauth_loopback::{LoopbackServer, PortMode};

const CALLBACK_TIMEOUT_SECS: u64 = 120;

/// LinkedIn OAuth scopes.
const LINKEDIN_SCOPES: &[&str] = &["openid", "profile", "w_member_social"];

/// YouTube OAuth scopes.
const YOUTUBE_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/youtube.upload",
    "https://www.googleapis.com/auth/youtube.readonly",
    "https://www.googleapis.com/auth/youtube.force-ssl",
];

/// Client credentials resolved from .corky.toml or env vars.
struct ClientCredentials {
    client_id: String,
    client_secret: String,
}

/// Resolve client credentials for a platform.
///
/// Resolution order per field: inline value > `_cmd` (shell command) > env var.
fn resolve_credentials(platform: Platform) -> Result<ClientCredentials> {
    match platform {
        Platform::LinkedIn => {
            // Try .corky.toml first (inline or _cmd)
            if let Some(cfg) = corky_config::try_load_config(None)
                && let Some(li) = &cfg.linkedin
            {
                let has_config = !li.client_id.is_empty()
                    || !li.client_id_cmd.is_empty()
                    || !li.client_secret.is_empty()
                    || !li.client_secret_cmd.is_empty();
                if has_config {
                    let client_id = crate::util::resolve_secret(
                        &li.client_id,
                        &li.client_id_cmd,
                        "LinkedIn client_id (check [linkedin] in .corky.toml)",
                    )?;
                    let client_secret = crate::util::resolve_secret(
                        &li.client_secret,
                        &li.client_secret_cmd,
                        "LinkedIn client_secret (check [linkedin] in .corky.toml)",
                    )?;
                    return Ok(ClientCredentials {
                        client_id,
                        client_secret,
                    });
                }
            }
            // Fall back to env vars
            let client_id = std::env::var("CORKY_LINKEDIN_CLIENT_ID")
                .context("LinkedIn client_id not found.\nSet [linkedin] in .corky.toml or CORKY_LINKEDIN_CLIENT_ID env var.")?;
            let client_secret = std::env::var("CORKY_LINKEDIN_CLIENT_SECRET")
                .context("LinkedIn client_secret not found.\nSet [linkedin] in .corky.toml or CORKY_LINKEDIN_CLIENT_SECRET env var.")?;
            Ok(ClientCredentials {
                client_id,
                client_secret,
            })
        }
        Platform::Youtube => {
            // Try .corky.toml first (inline or _cmd)
            if let Some(cfg) = corky_config::try_load_config(None)
                && let Some(yt) = &cfg.youtube
            {
                let has_config = !yt.client_id.is_empty()
                    || !yt.client_id_cmd.is_empty()
                    || !yt.client_secret.is_empty()
                    || !yt.client_secret_cmd.is_empty();
                if has_config {
                    let client_id = crate::util::resolve_secret(
                        &yt.client_id,
                        &yt.client_id_cmd,
                        "YouTube client_id (check [youtube] in .corky.toml)",
                    )?;
                    let client_secret = crate::util::resolve_secret(
                        &yt.client_secret,
                        &yt.client_secret_cmd,
                        "YouTube client_secret (check [youtube] in .corky.toml)",
                    )?;
                    return Ok(ClientCredentials {
                        client_id,
                        client_secret,
                    });
                }
            }
            // Fall back to env vars
            let client_id = std::env::var("CORKY_YOUTUBE_CLIENT_ID")
                .context("YouTube client_id not found.\nSet [youtube] in .corky.toml or CORKY_YOUTUBE_CLIENT_ID env var.")?;
            let client_secret = std::env::var("CORKY_YOUTUBE_CLIENT_SECRET")
                .context("YouTube client_secret not found.\nSet [youtube] in .corky.toml or CORKY_YOUTUBE_CLIENT_SECRET env var.")?;
            Ok(ClientCredentials {
                client_id,
                client_secret,
            })
        }
        _ => bail!("OAuth not yet implemented for {}", platform),
    }
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

/// Build the authorization URL for a platform.
pub fn build_auth_url(platform: Platform, redirect_uri: &str) -> Result<(String, String)> {
    let creds = resolve_credentials(platform)?;
    let state = generate_state();

    match platform {
        Platform::LinkedIn => {
            let scopes = LINKEDIN_SCOPES.join("%20");
            let url = format!(
                "https://www.linkedin.com/oauth/v2/authorization\
                 ?response_type=code\
                 &client_id={}\
                 &redirect_uri={}\
                 &state={}\
                 &scope={}",
                urlencode(&creds.client_id),
                urlencode(redirect_uri),
                urlencode(&state),
                scopes,
            );
            Ok((url, state))
        }
        Platform::Youtube => {
            let scopes = YOUTUBE_SCOPES.join("%20");
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
                urlencode(redirect_uri),
                urlencode(&state),
                scopes,
            );
            Ok((url, state))
        }
        _ => bail!("Auth URL generation not yet implemented for {}", platform),
    }
}

/// Percent-encode a string for use in application/x-www-form-urlencoded bodies.
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

/// Parse callback query string, returning (code, state).
pub fn parse_callback(query: &str) -> Result<(String, String)> {
    let mut code = None;
    let mut state = None;
    let mut error = None;

    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("");
        match key {
            "code" => code = Some(val.to_string()),
            "state" => state = Some(val.to_string()),
            "error" => error = Some(val.to_string()),
            "error_description" => {
                if error.is_some() {
                    error = Some(format!("{}: {}", error.unwrap(), val.replace('+', " ")));
                }
            }
            _ => {}
        }
    }

    if let Some(err) = error {
        bail!("OAuth error: {}", err);
    }

    let code = code.ok_or_else(|| anyhow::anyhow!("Callback missing 'code' parameter"))?;
    let state = state.ok_or_else(|| anyhow::anyhow!("Callback missing 'state' parameter"))?;

    Ok((code, state))
}

/// Exchange authorization code for tokens.
fn exchange_code(platform: Platform, code: &str, redirect_uri: &str) -> Result<StoredToken> {
    let creds = resolve_credentials(platform)?;

    match platform {
        Platform::LinkedIn => {
            let body_str = format!(
                "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&client_secret={}",
                urlencode(code),
                urlencode(redirect_uri),
                urlencode(&creds.client_id),
                urlencode(&creds.client_secret),
            );
            let resp = match ureq::post("https://www.linkedin.com/oauth/v2/accessToken")
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
                scopes: LINKEDIN_SCOPES.iter().map(|s| s.to_string()).collect(),
                platform: platform.to_string(),
            })
        }
        Platform::Youtube => {
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
                scopes: YOUTUBE_SCOPES.iter().map(|s| s.to_string()).collect(),
                platform: platform.to_string(),
            })
        }
        _ => bail!("Token exchange not yet implemented for {}", platform),
    }
}

/// Run the full OAuth flow: open browser, wait for callback, exchange code, store token.
pub fn run(platform: Platform, profile_name: Option<&str>) -> Result<()> {
    let port_mode = match platform {
        Platform::Youtube => PortMode::OptInEphemeralFallback,
        Platform::LinkedIn => PortMode::FixedOnly,
        _ => PortMode::FixedOnly,
    };
    let callback = LoopbackServer::bind(&platform.to_string(), port_mode)?;
    let redirect_uri = callback.redirect_uri().to_string();
    let (auth_url, expected_state) = build_auth_url(platform, &redirect_uri)?;

    notify_oauth(&platform.to_string());
    println!("Opening browser for {} authorization...", platform);
    println!("If the browser doesn't open, visit:\n  {}\n", auth_url);

    if open::that(&auth_url).is_err() {
        eprintln!("Could not open browser automatically.");
    }

    println!("Waiting for callback on {}...", redirect_uri);
    let callback = callback.recv_callback(CALLBACK_TIMEOUT_SECS)?;
    let code = callback.code.clone();
    let state = callback.state.clone();
    callback.respond_text("Authorization successful! You can close this tab.");

    // Verify state (CSRF protection)
    if state != expected_state {
        bail!(
            "State mismatch (CSRF protection). Expected '{}', got '{}'",
            expected_state,
            state
        );
    }

    // Exchange code for token
    println!("Exchanging authorization code...");
    let token = exchange_code(platform, &code, &redirect_uri)?;

    // Get user URN / channel ID
    let urn = match platform {
        Platform::LinkedIn => {
            let urn = super::linkedin::get_user_urn(&token.access_token)?;
            println!("Authenticated as URN: {}", urn);
            urn
        }
        Platform::Youtube => {
            let channel_id = super::youtube::get_channel_id(&token.access_token)?;
            println!("Authenticated as channel: {}", channel_id);
            channel_id
        }
        _ => bail!("URN retrieval not yet implemented for {}", platform),
    };

    // Update profiles.toml if profile name given
    if let Some(name) = profile_name {
        update_profile_urn(name, platform, &urn)?;
    }

    // Store token
    let mut store = TokenStore::load()?;
    store.upsert(urn.clone(), token);
    store.save()?;

    println!("Token stored for URN: {}", urn);
    Ok(())
}

/// Update the URN in .corky.toml [profiles] section for a given profile/platform.
///
/// Uses toml_edit for format-preserving writes.
fn update_profile_urn(profile_name: &str, platform: Platform, urn: &str) -> Result<()> {
    let path = crate::resolve::corky_toml();
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    // Ensure [profiles] table exists
    if !doc.contains_key("profiles") {
        doc.insert("profiles", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let profiles = doc["profiles"].as_table_mut().unwrap();

    // Ensure [profiles.<name>] table exists
    if !profiles.contains_key(profile_name) {
        profiles.insert(
            profile_name,
            toml_edit::Item::Table(toml_edit::Table::new()),
        );
    }
    let profile = profiles[profile_name].as_table_mut().unwrap();

    let platform_key = platform.as_str();
    // Ensure [profiles.<name>.<platform>] table exists
    if !profile.contains_key(platform_key) {
        let mut table = toml_edit::Table::new();
        table.insert("handle", toml_edit::value(""));
        profile.insert(platform_key, toml_edit::Item::Table(table));
    }
    let platform_table = profile[platform_key].as_table_mut().unwrap();
    platform_table.insert("urn", toml_edit::value(urn));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, doc.to_string())?;
    println!(
        "Updated .corky.toml: profiles.{}.{}.urn = {}",
        profile_name, platform_key, urn
    );
    Ok(())
}
