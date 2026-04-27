//! `corky social chat send` — send a message to a Google Chat space.

use anyhow::{Result, bail};

use crate::filter::gmail_auth;

const CHAT_API: &str = "https://chat.googleapis.com/v1/spaces";

/// Send a text message to a Google Chat space.
///
/// `space` may be the full resource name (`spaces/XXXXXXXXX`) or just the ID (`XXXXXXXXX`).
pub fn send(space: &str, message: &str, account: Option<&str>) -> Result<()> {
    let space_id = normalize_space(space);
    let token =
        gmail_auth::get_access_token_for_user(Some("default"), gmail_auth::CHAT_SCOPE, account)?;

    let url = format!("{}/{}/messages", CHAT_API, space_id);
    let body = serde_json::json!({ "text": message });

    eprintln!("Sending message to spaces/{}...", space_id);

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(&body);

    match resp {
        Ok(r) => {
            let result: serde_json::Value = r.into_json()?;
            let name = result["name"].as_str().unwrap_or("(unknown)");
            println!("Message sent: {}", name);
            Ok(())
        }
        Err(ureq::Error::Status(401, _)) => {
            bail!("Chat API: unauthorized (401). Re-run `corky filter auth`.")
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Chat API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Strip the `spaces/` prefix if present; return just the space ID.
fn normalize_space(space: &str) -> &str {
    space.strip_prefix("spaces/").unwrap_or(space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_space_with_prefix() {
        assert_eq!(normalize_space("spaces/ABCDEFG"), "ABCDEFG");
    }

    #[test]
    fn test_normalize_space_without_prefix() {
        assert_eq!(normalize_space("ABCDEFG"), "ABCDEFG");
    }
}
