//! Google Search Console authentication.
//!
//! The supported path is user OAuth against a real Google account that already
//! has Search Console access. If `[gsc]` contains a service-account key, corky
//! can still attempt that token path opportunistically, but Search Console does
//! not reliably treat service-account emails as addable property users, so
//! failed or empty access probes fall back to user OAuth.

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::config::corky_config;
use crate::desktop_notify::notify_oauth;
use crate::social::token_store::{StoredToken, TokenStore};

/// OAuth2 scope for read-only Search Console access.
pub(crate) const GSC_SCOPE: &str = "https://www.googleapis.com/auth/webmasters.readonly";

const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const SA_JWT_LIFETIME_SECS: i64 = 3600;

// Reuse corky's existing Google desktop-app loopback redirect.
const REDIRECT_URI: &str = "http://127.0.0.1:8484/callback";
const CALLBACK_TIMEOUT_SECS: u64 = 120;

/// Deserialized service-account JSON key.
#[derive(Debug, Deserialize)]
pub(crate) struct ServiceAccountKey {
    pub client_email: String,
    pub private_key: String,
    #[serde(default)]
    pub token_uri: Option<String>,
}

/// JWT claims for a service-account token request.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SaClaims {
    pub iss: String,
    pub scope: String,
    pub aud: String,
    pub exp: i64,
    pub iat: i64,
}

/// In-process cache of an SA-minted access token.
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    expires_at: chrono::DateTime<Utc>,
}

static SA_TOKEN_CACHE: Mutex<Option<CachedToken>> = Mutex::new(None);

struct ClientCredentials {
    client_id: String,
    client_secret: String,
}

fn token_key(account: Option<&str>) -> String {
    match account {
        Some(name) => format!("gsc:{}", name),
        None => "gsc:default".to_string(),
    }
}

/// Acquire a valid access token for the Search Console API.
///
/// Selection order:
/// 1. Stored user token (valid).
/// 2. Refresh stored user token.
/// 3. Verified service-account access from `[gsc]` config (if configured).
/// 4. Full user-OAuth flow (opens browser).
pub fn get_access_token(account: Option<&str>) -> Result<String> {
    let key = token_key(account);
    let mut store = TokenStore::load()?;

    if let Some(token) = store.get_valid(&key) {
        return Ok(token.access_token.clone());
    }

    if let Some(token) = store.tokens.get(&key).cloned()
        && let Some(ref refresh) = token.refresh_token
    {
        println!("GSC token expired, refreshing...");
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

    if let Some(token) = try_service_account()? {
        return Ok(token);
    }

    let token = run_auth_flow()?;
    let access = token.access_token.clone();
    store.upsert(key, token);
    store.save()?;
    Ok(access)
}

/// Run the explicit user-OAuth flow and store the resulting token.
pub fn run_auth(account: Option<&str>) -> Result<()> {
    let key = token_key(account);
    let token = run_auth_flow()?;
    let mut store = TokenStore::load()?;
    store.upsert(key.clone(), token);
    store.save()?;
    println!("GSC token stored as '{}'", key);
    Ok(())
}

/// Attempt service-account auth. Returns `Ok(None)` if `[gsc]` is not configured
/// or if Search Console access cannot be confirmed for the minted token.
fn try_service_account() -> Result<Option<String>> {
    let cfg = match corky_config::try_load_config(None).and_then(|c| c.gsc) {
        Some(c) => c,
        None => return Ok(None),
    };
    if cfg.service_account_json.is_empty() && cfg.service_account_json_cmd.is_empty() {
        return Ok(None);
    }

    if let Some(token) = cached_token() {
        if let Err(err) = verify_service_account_access(&token) {
            clear_cached_token();
            eprintln!("{}", format_service_account_fallback(&err));
            return Ok(None);
        }
        return Ok(Some(token));
    }

    let key_json = crate::util::resolve_secret(
        &cfg.service_account_json,
        &cfg.service_account_json_cmd,
        "gsc service_account_json (check [gsc] in .corky.toml)",
    )?;
    let sa = parse_service_account_key(&key_json)?;
    let jwt = sign_sa_jwt(&sa)?;
    let fetched = match exchange_sa_jwt(&sa, &jwt) {
        Ok(token) => token,
        Err(err) => {
            eprintln!("{}", format_service_account_fallback(&err));
            return Ok(None);
        }
    };
    if let Err(err) = verify_service_account_access(&fetched.access_token) {
        eprintln!("{}", format_service_account_fallback(&err));
        return Ok(None);
    }
    store_token(&fetched);
    Ok(Some(fetched.access_token))
}

/// Parse a service-account key JSON blob.
pub(crate) fn parse_service_account_key(json: &str) -> Result<ServiceAccountKey> {
    let sa: ServiceAccountKey = serde_json::from_str(json)
        .context("failed to parse service account JSON (expected Google SA key file)")?;
    if sa.client_email.is_empty() || sa.private_key.is_empty() {
        bail!("service account JSON missing client_email or private_key");
    }
    Ok(sa)
}

/// Sign a JWT for the `jwt-bearer` grant exchange.
pub(crate) fn sign_sa_jwt(sa: &ServiceAccountKey) -> Result<String> {
    let now = Utc::now().timestamp();
    let claims = SaClaims {
        iss: sa.client_email.clone(),
        scope: GSC_SCOPE.to_string(),
        aud: sa
            .token_uri
            .clone()
            .unwrap_or_else(|| GOOGLE_TOKEN_URI.to_string()),
        exp: now + SA_JWT_LIFETIME_SECS,
        iat: now,
    };
    let key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
        .context("failed to load SA private key (expected RSA PEM)")?;
    encode(&Header::new(Algorithm::RS256), &claims, &key).context("failed to sign SA JWT")
}

/// Exchange a signed JWT for an access token.
fn exchange_sa_jwt(sa: &ServiceAccountKey, jwt: &str) -> Result<CachedToken> {
    let token_uri = sa.token_uri.as_deref().unwrap_or(GOOGLE_TOKEN_URI);
    let body = format!(
        "grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer&assertion={}",
        urlencode(jwt),
    );
    let resp = match ureq::post(token_uri)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!(
                "gsc SA token exchange failed (HTTP {}): {}\n\
                 Hint: Search Console property access usually requires a real Google \
                 account with access. If the service-account identity cannot be used \
                 there, run `corky gsc auth` instead.",
                status,
                err_body
            );
        }
        Err(e) => return Err(e.into()),
    };
    let body: serde_json::Value = resp.into_json()?;
    parse_sa_token_response(&body)
}

fn parse_sa_token_response(body: &serde_json::Value) -> Result<CachedToken> {
    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("SA token response missing access_token"))?
        .to_string();
    let expires_in = body["expires_in"].as_i64().unwrap_or(3600);
    Ok(CachedToken {
        access_token,
        expires_at: Utc::now() + Duration::seconds(expires_in - 60),
    })
}

fn cached_token() -> Option<String> {
    let guard = SA_TOKEN_CACHE.lock().ok()?;
    let cached = guard.as_ref()?;
    if cached.expires_at > Utc::now() {
        Some(cached.access_token.clone())
    } else {
        None
    }
}

fn store_token(token: &CachedToken) {
    if let Ok(mut guard) = SA_TOKEN_CACHE.lock() {
        *guard = Some(token.clone());
    }
}

fn clear_cached_token() {
    if let Ok(mut guard) = SA_TOKEN_CACHE.lock() {
        *guard = None;
    }
}

fn verify_service_account_access(token: &str) -> Result<()> {
    let sites = crate::gsc::sites::list_sites(token)?;
    ensure_service_account_sites(&sites)
}

fn ensure_service_account_sites(sites: &[crate::gsc::sites::SiteEntry]) -> Result<()> {
    if sites.is_empty() {
        bail!(
            "service-account token succeeded, but Search Console returned no visible \
             properties. Search Console user management expects a valid Google \
             Account email, so service-account identities are not a reliable \
             property-access path."
        );
    }
    Ok(())
}

fn format_service_account_fallback(err: &anyhow::Error) -> String {
    format!(
        "GSC service-account auth is unavailable: {}\n\
         Falling back to user OAuth via `corky gsc auth`.",
        err
    )
}

/// Resolve OAuth2 client credentials from `.corky.toml` `[gmail]` section or env vars.
/// GSC user-OAuth reuses the same Google project credentials as Gmail/Calendar.
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

fn generate_state() -> String {
    use std::time::SystemTime;
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}", nonce)
}

fn run_auth_flow() -> Result<StoredToken> {
    let creds = resolve_credentials()?;
    let state = generate_state();

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
        urlencode(REDIRECT_URI),
        urlencode(&state),
        urlencode(GSC_SCOPE),
    );

    notify_oauth("Google Search Console");
    println!("Opening browser for Google Search Console authorization...");
    println!("If the browser doesn't open, visit:\n  {}\n", url);

    if open::that(&url).is_err() {
        eprintln!("Could not open browser automatically.");
    }

    println!("Waiting for callback on {}...", REDIRECT_URI);
    let server = tiny_http::Server::http("127.0.0.1:8484")
        .map_err(|e| anyhow!("Failed to start callback server: {}", e))?;

    let request = server
        .recv_timeout(std::time::Duration::from_secs(CALLBACK_TIMEOUT_SECS))
        .map_err(|e| anyhow!("Callback server error: {}", e))?
        .ok_or_else(|| {
            anyhow!(
                "Timed out waiting for OAuth callback ({}s)",
                CALLBACK_TIMEOUT_SECS
            )
        })?;

    let url_str = request.url().to_string();
    let query = url_str.split('?').nth(1).unwrap_or("");
    let (code, cb_state) = crate::social::auth::parse_callback(query)?;

    let response = tiny_http::Response::from_string(
        "Google Search Console authorization successful! You can close this tab.",
    );
    let _ = request.respond(response);

    if cb_state != state {
        bail!(
            "State mismatch (CSRF). Expected '{}', got '{}'",
            state,
            cb_state
        );
    }

    println!("Exchanging authorization code...");
    exchange_code(&creds, &code)
}

fn exchange_code(creds: &ClientCredentials, code: &str) -> Result<StoredToken> {
    let body_str = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&client_secret={}",
        urlencode(code),
        urlencode(REDIRECT_URI),
        urlencode(&creds.client_id),
        urlencode(&creds.client_secret),
    );

    let resp = match ureq::post(GOOGLE_TOKEN_URI)
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
    parse_user_token_response(&body)
}

fn refresh_access_token(refresh_token: &str) -> Result<StoredToken> {
    let creds = resolve_credentials()?;
    let body_str = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
        urlencode(refresh_token),
        urlencode(&creds.client_id),
        urlencode(&creds.client_secret),
    );

    let resp = match ureq::post(GOOGLE_TOKEN_URI)
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
    let mut token = parse_user_token_response(&body)?;
    token.refresh_token = Some(refresh_token.to_string());
    Ok(token)
}

fn parse_user_token_response(body: &serde_json::Value) -> Result<StoredToken> {
    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing access_token in response"))?
        .to_string();
    let expires_in = body["expires_in"].as_i64().unwrap_or(3600);
    let refresh_token = body["refresh_token"].as_str().map(|s| s.to_string());

    Ok(StoredToken {
        access_token,
        refresh_token,
        expires_at: Utc::now() + Duration::seconds(expires_in),
        scopes: vec![GSC_SCOPE.to_string()],
        platform: "gsc".to_string(),
    })
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{DecodingKey, Validation, decode_header};

    // RSA-2048 test key. Generated once with `openssl genpkey` for these tests.
    // Not a real credential — never used outside unit tests.
    const TEST_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCslOn49P+2UuHH
jIFyGvWn+bOlf4gmFJRbJzErUNlfdVWbVQa8X5ItJRJbQ8BLQlYc9sfH2sviGm/m
y9itwcdQo0aPW5or+g1VL89higbcMnrZwLeWX66U9CFIrYNbzmcVO1r7u0xuAbmT
m0JtMhKFkvWDHJ6V7hcqNLw6N3CW3wDYwmhsUgNZsk5RvJb7h52W2/OWYkTSdEh8
InMmjbY1JcT1v4jU16nf0KsqxYY2dBle0QaO9vCGpJcn7SULu2k+bVsqpS7cd8fk
3LUMSHbYwdjetXqQOdLGnoIavyS5ZPbAOZBoFWyIveTSuaz1quk7T4dwNCzigiCQ
ELvziUyNAgMBAAECggEAAcF7FuYRMu7pbqEKkRlenfCfqAOh5DdJ4hqGOMlYCM1W
C2BEUwbK4ywxIV4RVwKsAxvsoOckM17u/ANfZzclOrgKh/tX2HIxEAiOPNENbFCU
KOJ5y60aBthr/UQMpdK2AESMEqsPZkAcvfhyps8/Wn4KAuU35NEZCbwjHRmCyvoi
CvKcXDz3xYNEfweGAD34SNxnjv+54seEt4F9AHGgE4fJiKRJbhCOKZV74KoC2I23
xMiiyoEWNUkMkvVa51bTZU69pmD/JP2mpht+9XAf0+tCC9xs6nm+uTNEb9UImYQo
Mlahak0+lGywfF6ky9gtcFhQYREXmksf7PzQ8939WQKBgQDjpSD+r/mL/vOqY5tW
zDJqoi8TyJzKHv6lxXuv3GeLAidp4/MAmal7BoduuNOSCD+f3+z5nUw12GZO++mz
mvP9p8SaIdtp3HC/XGJTwhUbdGaqn+s0c3dIDpKY/GaG/AOLTXSmtlRcYjr9/Ke+
Tv3StBAez1Ycf0y/5pqhmuo7pQKBgQDCE/3Tw7TmIc8NBN8adKW5BRzTvxJo8O23
0lAtxDJyjfJVJUHl4GGkMEp/rQU+WrgL4HOGH4AHbJWvWyu3DaGbVt7oV+wHPJQV
AiKfIalqip9Yby3NluQPru5jwMTqs/iddbcW1bA29e8dVa8xXgX1yauY3DLyxWmz
77pHAZUYyQKBgGrpc5yJkv6Px2o/i4XxMsBn2QpGjnRSqC+8lsFaFvrvEQmnN8oR
YMpZn6N9hEeyPgdcyFPW7yLetfXkU7a5UFvRvgDRY9XM5NrKjZdesEELougBYRpq
HBwoU+srpw9ALn3u65kcSnR04dXFIha7zHN3g5aks4GAu8/ogrjhI57NAoGBAIfq
CDBtNhqUQrQTXUrhtc1Ez1Na1EG5uECrgIsMg2fGEJegZ+3cnYSmbQXM3Yc1cP6g
SUb8eGS6nnkXmB2x5iMrSx/bsue+fNXZkPVwVXzPZ5g/BAyeR0jUcQ5ayYy0TL+4
2GedbrKOuM4KW45vEi129j0uuF9b8RKaKBHiAdBBAoGBAICkqO9dYrRikjlafSup
UKIqY6wEyLTpWCIWRwx5YdohBdBoS9vbUgedIZ453BPXhDBQVYI2rwJKyYORQx2t
SMygu5AtZbqLXegvq/ty/A4bXhjeikupxo7DxRN69cRmk+6tg/cPTR6MXn2ZctV2
jzyrXB+qTE8wF1xUHoaCJKgB
-----END PRIVATE KEY-----
";

    fn fixture_sa_json() -> String {
        serde_json::json!({
            "type": "service_account",
            "project_id": "test-project",
            "client_email": "sa@test.iam.gserviceaccount.com",
            "private_key": TEST_PRIVATE_KEY,
            "private_key_id": "abc123",
            "token_uri": "https://oauth2.googleapis.com/token"
        })
        .to_string()
    }

    #[test]
    fn parses_service_account_key() {
        let sa = parse_service_account_key(&fixture_sa_json()).unwrap();
        assert_eq!(sa.client_email, "sa@test.iam.gserviceaccount.com");
        assert!(sa.private_key.contains("BEGIN PRIVATE KEY"));
        assert_eq!(
            sa.token_uri.as_deref(),
            Some("https://oauth2.googleapis.com/token")
        );
    }

    #[test]
    fn rejects_sa_key_missing_fields() {
        let bad = serde_json::json!({
            "client_email": "",
            "private_key": "",
        })
        .to_string();
        assert!(parse_service_account_key(&bad).is_err());
    }

    #[test]
    fn signs_sa_jwt_with_expected_claims() {
        let sa = parse_service_account_key(&fixture_sa_json()).unwrap();
        let jwt = sign_sa_jwt(&sa).expect("signing should succeed");

        let header = decode_header(&jwt).unwrap();
        assert_eq!(header.alg, Algorithm::RS256);

        let mut validation = Validation::new(Algorithm::RS256);
        validation.insecure_disable_signature_validation();
        validation.set_audience(&[GOOGLE_TOKEN_URI]);
        validation.required_spec_claims.clear();
        let data = jsonwebtoken::decode::<SaClaims>(
            &jwt,
            &DecodingKey::from_secret(b"unused"),
            &validation,
        )
        .expect("decode (signature validation disabled)");

        let c = data.claims;
        assert_eq!(c.iss, "sa@test.iam.gserviceaccount.com");
        assert_eq!(c.scope, GSC_SCOPE);
        assert_eq!(c.aud, GOOGLE_TOKEN_URI);
        assert_eq!(c.exp - c.iat, SA_JWT_LIFETIME_SECS);
    }

    #[test]
    fn parses_sa_token_response() {
        let body = serde_json::json!({
            "access_token": "ya29.test-sa",
            "expires_in": 3600,
            "token_type": "Bearer"
        });
        let t = parse_sa_token_response(&body).unwrap();
        assert_eq!(t.access_token, "ya29.test-sa");
        assert!(t.expires_at > Utc::now());
    }

    #[test]
    fn token_key_default() {
        assert_eq!(token_key(None), "gsc:default");
    }

    #[test]
    fn token_key_named() {
        assert_eq!(token_key(Some("brian")), "gsc:brian");
    }

    #[test]
    fn parses_user_token_response() {
        let body = serde_json::json!({
            "access_token": "ya29.test-user",
            "expires_in": 3600,
            "refresh_token": "1//test-refresh",
            "token_type": "Bearer"
        });
        let token = parse_user_token_response(&body).unwrap();
        assert_eq!(token.access_token, "ya29.test-user");
        assert_eq!(token.refresh_token.as_deref(), Some("1//test-refresh"));
        assert_eq!(token.platform, "gsc");
        assert_eq!(token.scopes, vec![GSC_SCOPE.to_string()]);
    }

    #[test]
    fn rejects_service_account_probe_with_no_sites() {
        let sites: Vec<crate::gsc::sites::SiteEntry> = vec![];
        let err = ensure_service_account_sites(&sites)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no visible properties"));
        assert!(err.contains("valid Google Account"));
    }

    #[test]
    fn accepts_service_account_probe_with_visible_sites() {
        let sites = vec![crate::gsc::sites::SiteEntry {
            site_url: "sc-domain:example.com".to_string(),
            permission_level: "siteOwner".to_string(),
        }];
        ensure_service_account_sites(&sites).unwrap();
    }

    #[test]
    fn fallback_message_points_to_user_oauth() {
        let err = anyhow!("probe failed");
        let msg = format_service_account_fallback(&err);
        assert!(msg.contains("Falling back to user OAuth"));
        assert!(msg.contains("corky gsc auth"));
    }
}
