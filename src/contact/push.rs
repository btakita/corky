//! Push local contacts to external platforms (Google Contacts).
//!
//! Uses Google People API to create or update contacts.
//! Reuses Gmail OAuth credentials with `contacts` scope.
//! Tracks Google `resourceName` per contact in `.sync-state.json` for update-vs-create.

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use std::collections::BTreeMap;

use crate::cal::auth::urlencode_pub as urlencode;
use crate::config::contact::load_contacts;
use crate::config::corky_config;
use crate::desktop_notify::notify_oauth;
use crate::oauth_loopback::{LoopbackServer, PortMode};
use crate::resolve;
use crate::social::token_store::{StoredToken, TokenStore};

const CALLBACK_TIMEOUT_SECS: u64 = 120;
const PEOPLE_SCOPE: &str = "https://www.googleapis.com/auth/contacts";

struct ClientCredentials {
    client_id: String,
    client_secret: String,
}

/// Contact fields extracted from local data for push to Google.
struct ContactPayload {
    display_name: String,
    emails: Vec<String>,
    organization: Option<String>,
    title: Option<String>,
    phone: Option<String>,
    linkedin_url: Option<String>,
    notes: Option<String>,
}

/// Google People API resource mapping stored in .sync-state.json.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct PeopleSyncState {
    /// Maps local contact name → Google People resourceName (e.g., "people/c12345")
    #[serde(default)]
    google_resource_names: BTreeMap<String, String>,
}

// --- OAuth ---

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

fn token_key() -> String {
    "people:default".to_string()
}

fn generate_state() -> String {
    use std::time::SystemTime;
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}", nonce)
}

/// Get a valid People API access token, refreshing or running full auth flow if needed.
pub fn get_access_token() -> Result<String> {
    let key = token_key();
    let mut store = TokenStore::load()?;

    if let Some(token) = store.get_valid(&key) {
        return Ok(token.access_token.clone());
    }

    if let Some(token) = store.tokens.get(&key).cloned()
        && let Some(ref refresh) = token.refresh_token
    {
        println!("People API token expired, refreshing...");
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

/// Run explicit People API OAuth2 authentication.
pub fn run_auth() -> Result<()> {
    let key = token_key();
    let token = run_auth_flow()?;
    let mut store = TokenStore::load()?;
    store.upsert(key.clone(), token);
    store.save()?;
    println!("People API token stored as '{}'", key);
    Ok(())
}

fn run_auth_flow() -> Result<StoredToken> {
    let creds = resolve_credentials()?;
    let state = generate_state();
    let callback = LoopbackServer::bind("Google Contacts", PortMode::EphemeralFallback)?;
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
        urlencode(PEOPLE_SCOPE),
    );

    notify_oauth("Google Contacts");
    println!("Opening browser for Google Contacts authorization...");
    println!("If the browser doesn't open, visit:\n  {}\n", url);

    if open::that(&url).is_err() {
        eprintln!("Could not open browser automatically.");
    }

    println!("Waiting for callback on {}...", redirect_uri);
    let callback = callback.recv_callback(CALLBACK_TIMEOUT_SECS)?;
    let code = callback.code.clone();
    let cb_state = callback.state.clone();
    callback.respond_text("Google Contacts authorization successful! You can close this tab.");

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
        scopes: vec![PEOPLE_SCOPE.to_string()],
        platform: "people".to_string(),
    })
}

// --- Sync state ---

fn sync_state_path() -> std::path::PathBuf {
    resolve::data_dir().join(".people-sync-state.json")
}

fn load_sync_state() -> Result<PeopleSyncState> {
    let path = sync_state_path();
    if !path.exists() {
        return Ok(PeopleSyncState::default());
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&content)?)
}

fn save_sync_state(state: &PeopleSyncState) -> Result<()> {
    let path = sync_state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, content)?;
    Ok(())
}

// --- Field extraction from AGENTS.md ---

fn extract_contact_payload(name: &str, emails: &[String]) -> ContactPayload {
    let contacts_dir = resolve::contacts_dir();
    let contact_dir = contacts_dir.join(name);
    let agents_path = contact_dir.join("AGENTS.md");
    let agents_path = if agents_path.exists() {
        agents_path
    } else {
        contact_dir.join("CLAUDE.md")
    };

    let mut display_name = name
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().to_string() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let mut organization = None;
    let mut title = None;
    let mut phone = None;
    let mut linkedin_url = None;
    let mut notes_lines = Vec::new();

    if let Ok(content) = std::fs::read_to_string(&agents_path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("**Role:**") {
                let rest = rest.trim();
                if let Some(idx) = rest.find(" at ") {
                    title = Some(rest[..idx].trim().to_string());
                    organization = Some(rest[idx + 4..].trim().to_string());
                } else {
                    title = Some(rest.to_string());
                }
            } else if let Some(rest) = trimmed
                .strip_prefix("**Organization:**")
                .or_else(|| trimmed.strip_prefix("**Company:**"))
                .or_else(|| trimmed.strip_prefix("**Org:**"))
            {
                organization = Some(rest.trim().to_string());
            } else if let Some(rest) = trimmed.strip_prefix("**Title:**") {
                title = Some(rest.trim().to_string());
            } else if let Some(rest) = trimmed.strip_prefix("**Phone:**") {
                phone = Some(rest.trim().to_string());
            } else if let Some(rest) = trimmed
                .strip_prefix("**LinkedIn:**")
                .or_else(|| trimmed.strip_prefix("- LinkedIn:"))
            {
                linkedin_url = Some(rest.trim().to_string());
            }
        }
        // Use first heading as display name if it's a real name (not template)
        if let Some(heading) = content.lines().find(|l| l.starts_with("# ")) {
            let h = heading.strip_prefix("# ").unwrap_or(heading).trim();
            // "Contact: Anil Podduturi" → "Anil Podduturi"
            let h = h.strip_prefix("Contact:").map(|s| s.trim()).unwrap_or(h);
            if !h.is_empty() {
                display_name = h.to_string();
            }
        }
    }

    // Build notes from org context
    if let Some(ref li) = linkedin_url {
        notes_lines.push(format!("LinkedIn: {}", li));
    }

    ContactPayload {
        display_name,
        emails: emails.to_vec(),
        organization,
        title,
        phone,
        linkedin_url,
        notes: if notes_lines.is_empty() {
            None
        } else {
            Some(notes_lines.join("\n"))
        },
    }
}

/// Build Google People API person JSON from local contact data.
fn build_person_json(payload: &ContactPayload) -> serde_json::Value {
    let mut person = serde_json::json!({
        "names": [{
            "givenName": payload.display_name.split(' ').next().unwrap_or(""),
            "familyName": payload.display_name.split(' ').skip(1).collect::<Vec<_>>().join(" "),
        }],
    });

    if !payload.emails.is_empty() {
        let emails: Vec<serde_json::Value> = payload
            .emails
            .iter()
            .enumerate()
            .map(|(i, e)| {
                serde_json::json!({
                    "value": e,
                    "type": if i == 0 { "work" } else { "other" },
                })
            })
            .collect();
        person["emailAddresses"] = serde_json::Value::Array(emails);
    }

    if let Some(ref org) = payload.organization {
        let mut org_json = serde_json::json!({ "name": org });
        if let Some(ref t) = payload.title {
            org_json["title"] = serde_json::Value::String(t.clone());
        }
        person["organizations"] = serde_json::json!([org_json]);
    } else if let Some(ref t) = payload.title {
        person["organizations"] = serde_json::json!([{ "title": t }]);
    }

    if let Some(ref phone) = payload.phone {
        person["phoneNumbers"] = serde_json::json!([{
            "value": phone,
            "type": "mobile",
        }]);
    }

    if let Some(ref url) = payload.linkedin_url {
        person["urls"] = serde_json::json!([{
            "value": url,
            "type": "profile",
        }]);
    }

    if let Some(ref notes) = payload.notes {
        person["biographies"] = serde_json::json!([{
            "value": notes,
            "contentType": "TEXT_PLAIN",
        }]);
    }

    person
}

// --- People API calls ---

fn create_contact(token: &str, person: &serde_json::Value) -> Result<String> {
    let resp = match ureq::post("https://people.googleapis.com/v1/people:createContact")
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&person.to_string())
    {
        Ok(r) => r,
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Create contact failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => return Err(e.into()),
    };

    let body: serde_json::Value = resp.into_json()?;
    let resource_name = body["resourceName"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing resourceName in create response"))?
        .to_string();
    Ok(resource_name)
}

fn update_contact(token: &str, resource_name: &str, person: &serde_json::Value) -> Result<()> {
    // First get the current etag
    let get_url = format!(
        "https://people.googleapis.com/v1/{}?personFields=names",
        resource_name
    );
    let get_resp = match ureq::get(&get_url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => {
            // Resource gone — create instead
            bail!(
                "Contact {} not found on Google — will create instead",
                resource_name
            );
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Get contact failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => return Err(e.into()),
    };

    let existing: serde_json::Value = get_resp.into_json()?;
    let etag = existing["etag"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing etag for {}", resource_name))?;

    let mut update_body = person.clone();
    update_body["etag"] = serde_json::Value::String(etag.to_string());

    let update_fields = "names,emailAddresses,organizations,phoneNumbers,urls,biographies";
    let update_url = format!(
        "https://people.googleapis.com/v1/{}:updateContact?updatePersonFields={}",
        resource_name, update_fields
    );

    match ureq::request("PATCH", &update_url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&update_body.to_string())
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Update contact failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => Err(e.into()),
    }
}

// --- Public API ---

/// Discover all contacts from both .corky.toml and the contacts/ directory.
/// Returns a merged map: config contacts take precedence, filesystem contacts fill gaps.
fn discover_contacts() -> Result<BTreeMap<String, crate::config::contact::Contact>> {
    let mut contacts = load_contacts(None)?;

    // Also discover from contacts/ directory (contacts may exist without .corky.toml entries)
    let contacts_dir = resolve::contacts_dir();
    if contacts_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&contacts_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
                && !contacts.contains_key(name)
            {
                let mut emails = Vec::new();
                let agents_path = path.join("AGENTS.md");
                let agents_path = if agents_path.exists() {
                    agents_path
                } else {
                    path.join("CLAUDE.md")
                };
                if let Ok(content) = std::fs::read_to_string(&agents_path) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if let Some(rest) = trimmed.strip_prefix("**Email:**") {
                            let email = rest.split_whitespace().next().unwrap_or("").to_string();
                            if email.contains('@') {
                                emails.push(email);
                            }
                        }
                    }
                }
                contacts.insert(
                    name.to_string(),
                    crate::config::contact::Contact {
                        emails,
                        ..Default::default()
                    },
                );
            }
        }
    }

    Ok(contacts)
}

/// Push all local contacts to Google Contacts.
pub fn run_google(names: &[String]) -> Result<()> {
    let contacts = discover_contacts()?;
    if contacts.is_empty() {
        println!("No contacts found");
        return Ok(());
    }

    let token = get_access_token()?;
    let mut sync_state = load_sync_state()?;
    let mut created = 0u32;
    let mut updated = 0u32;
    let mut errors = 0u32;

    let to_push: Vec<(&String, &crate::config::contact::Contact)> = if names.is_empty() {
        contacts.iter().collect()
    } else {
        contacts
            .iter()
            .filter(|(name, _)| names.iter().any(|n| n == *name))
            .collect()
    };

    if to_push.is_empty() {
        println!("No matching contacts found");
        return Ok(());
    }

    println!("Pushing {} contacts to Google...", to_push.len());

    for (name, contact) in &to_push {
        let payload = extract_contact_payload(name, &contact.emails);
        let person = build_person_json(&payload);

        if let Some(resource_name) = sync_state.google_resource_names.get(*name) {
            // Update existing
            match update_contact(&token, resource_name, &person) {
                Ok(()) => {
                    println!("  Updated: {} ({})", payload.display_name, resource_name);
                    updated += 1;
                }
                Err(e) => {
                    // If not found, try create
                    if e.to_string().contains("not found") {
                        match create_contact(&token, &person) {
                            Ok(rn) => {
                                println!("  Created: {} ({})", payload.display_name, rn);
                                sync_state
                                    .google_resource_names
                                    .insert(name.to_string(), rn);
                                created += 1;
                            }
                            Err(e2) => {
                                eprintln!("  Error creating {}: {}", name, e2);
                                errors += 1;
                            }
                        }
                    } else {
                        eprintln!("  Error updating {}: {}", name, e);
                        errors += 1;
                    }
                }
            }
        } else {
            // Create new
            match create_contact(&token, &person) {
                Ok(resource_name) => {
                    println!("  Created: {} ({})", payload.display_name, resource_name);
                    sync_state
                        .google_resource_names
                        .insert(name.to_string(), resource_name);
                    created += 1;
                }
                Err(e) => {
                    eprintln!("  Error creating {}: {}", name, e);
                    errors += 1;
                }
            }
        }
    }

    save_sync_state(&sync_state)?;

    println!();
    println!(
        "Done: {} created, {} updated, {} errors",
        created, updated, errors
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_contact_payload_basic() {
        let payload = extract_contact_payload("john-doe", &["john@example.com".to_string()]);
        assert_eq!(payload.display_name, "John Doe");
        assert_eq!(payload.emails, vec!["john@example.com"]);
    }

    #[test]
    fn test_build_person_json_minimal() {
        let payload = ContactPayload {
            display_name: "John Doe".to_string(),
            emails: vec!["john@example.com".to_string()],
            organization: None,
            title: None,
            phone: None,
            linkedin_url: None,
            notes: None,
        };
        let json = build_person_json(&payload);
        assert_eq!(json["names"][0]["givenName"], "John");
        assert_eq!(json["names"][0]["familyName"], "Doe");
        assert_eq!(json["emailAddresses"][0]["value"], "john@example.com");
    }

    #[test]
    fn test_build_person_json_full() {
        let payload = ContactPayload {
            display_name: "Jane Smith".to_string(),
            emails: vec!["jane@work.com".to_string(), "jane@personal.com".to_string()],
            organization: Some("Acme Corp".to_string()),
            title: Some("Engineer".to_string()),
            phone: Some("+1-555-1234".to_string()),
            linkedin_url: Some("https://linkedin.com/in/janesmith".to_string()),
            notes: Some("LinkedIn: https://linkedin.com/in/janesmith".to_string()),
        };
        let json = build_person_json(&payload);
        assert_eq!(json["names"][0]["givenName"], "Jane");
        assert_eq!(json["names"][0]["familyName"], "Smith");
        assert_eq!(json["emailAddresses"].as_array().unwrap().len(), 2);
        assert_eq!(json["organizations"][0]["name"], "Acme Corp");
        assert_eq!(json["organizations"][0]["title"], "Engineer");
        assert_eq!(json["phoneNumbers"][0]["value"], "+1-555-1234");
        assert_eq!(
            json["urls"][0]["value"],
            "https://linkedin.com/in/janesmith"
        );
    }

    #[test]
    fn test_token_key() {
        assert_eq!(token_key(), "people:default");
    }
}
