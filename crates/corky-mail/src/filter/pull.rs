//! Fetch and display Gmail filters via the Gmail Settings API.

use anyhow::{Context, Result, bail};

use super::gmail_auth;

/// Gmail filter as returned by the API.
#[derive(Debug, serde::Deserialize)]
struct FilterListResponse {
    #[serde(default)]
    filter: Vec<GmailFilter>,
}

#[derive(Debug, serde::Deserialize)]
struct GmailFilter {
    id: String,
    #[serde(default)]
    criteria: FilterCriteria,
    #[serde(default)]
    action: FilterAction,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterCriteria {
    from: Option<String>,
    to: Option<String>,
    subject: Option<String>,
    query: Option<String>,
    negated_query: Option<String>,
    has_attachment: Option<bool>,
    exclude_chats: Option<bool>,
    size: Option<u64>,
    size_comparison: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterAction {
    #[serde(default)]
    add_label_ids: Vec<String>,
    #[serde(default)]
    remove_label_ids: Vec<String>,
    forward: Option<String>,
}

pub fn run(account: Option<&str>) -> Result<()> {
    let access_token = gmail_auth::get_access_token(account)?;

    let resp = match ureq::get("https://gmail.googleapis.com/gmail/v1/users/me/settings/filters")
        .set("Authorization", &format!("Bearer {}", access_token))
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => {
            bail!(
                "Gmail API returned 401 Unauthorized.\n\
                 Try re-authenticating with: corky filter auth"
            );
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Gmail API error (HTTP {}): {}", status, err_body);
        }
        Err(e) => return Err(e.into()),
    };

    let body: FilterListResponse = resp
        .into_json()
        .context("Failed to parse Gmail filters response")?;

    if body.filter.is_empty() {
        println!("No filters found.");
        return Ok(());
    }

    println!("{} filter(s):\n", body.filter.len());

    for (i, filt) in body.filter.iter().enumerate() {
        println!("--- Filter {} (id: {}) ---", i + 1, filt.id);
        print_criteria(&filt.criteria);
        print_actions(&filt.action);
        println!();
    }

    Ok(())
}

fn print_criteria(c: &FilterCriteria) {
    if let Some(ref v) = c.from {
        println!("  from: {}", v);
    }
    if let Some(ref v) = c.to {
        println!("  to: {}", v);
    }
    if let Some(ref v) = c.subject {
        println!("  subject: {}", v);
    }
    if let Some(ref v) = c.query {
        println!("  query: {}", v);
    }
    if let Some(ref v) = c.negated_query {
        println!("  negated_query: {}", v);
    }
    if c.has_attachment == Some(true) {
        println!("  has_attachment: true");
    }
    if c.exclude_chats == Some(true) {
        println!("  exclude_chats: true");
    }
    if let Some(size) = c.size {
        let cmp = c.size_comparison.as_deref().unwrap_or("?");
        println!("  size: {} {}", cmp, size);
    }
}

fn print_actions(a: &FilterAction) {
    if !a.add_label_ids.is_empty() {
        println!("  add_labels: {}", a.add_label_ids.join(", "));
    }
    if !a.remove_label_ids.is_empty() {
        println!("  remove_labels: {}", a.remove_label_ids.join(", "));
    }
    if let Some(ref fwd) = a.forward {
        println!("  forward_to: {}", fwd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_filter_list() {
        let json = r#"{
            "filter": [
                {
                    "id": "abc123",
                    "criteria": {
                        "from": "test@example.com",
                        "hasAttachment": true
                    },
                    "action": {
                        "addLabelIds": ["Label_1", "STARRED"],
                        "removeLabelIds": ["INBOX"]
                    }
                }
            ]
        }"#;

        let resp: FilterListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.filter.len(), 1);
        let f = &resp.filter[0];
        assert_eq!(f.id, "abc123");
        assert_eq!(f.criteria.from.as_deref(), Some("test@example.com"));
        assert_eq!(f.criteria.has_attachment, Some(true));
        assert_eq!(f.action.add_label_ids, vec!["Label_1", "STARRED"]);
        assert_eq!(f.action.remove_label_ids, vec!["INBOX"]);
    }

    #[test]
    fn test_deserialize_empty_filter_list() {
        let json = r#"{}"#;
        let resp: FilterListResponse = serde_json::from_str(json).unwrap();
        assert!(resp.filter.is_empty());
    }

    #[test]
    fn test_deserialize_filter_with_forward() {
        let json = r#"{
            "filter": [
                {
                    "id": "def456",
                    "criteria": {
                        "from": "sender@example.com"
                    },
                    "action": {
                        "forward": "other@example.com",
                        "addLabelIds": ["STARRED"]
                    }
                }
            ]
        }"#;

        let resp: FilterListResponse = serde_json::from_str(json).unwrap();
        let f = &resp.filter[0];
        assert_eq!(f.action.forward.as_deref(), Some("other@example.com"));
        assert_eq!(f.action.add_label_ids, vec!["STARRED"]);
    }

    #[test]
    fn test_deserialize_filter_with_query() {
        let json = r#"{
            "filter": [
                {
                    "id": "ghi789",
                    "criteria": {
                        "query": "is:important",
                        "negatedQuery": "from:spam@example.com",
                        "excludeChats": true,
                        "size": 1048576,
                        "sizeComparison": "larger"
                    },
                    "action": {
                        "addLabelIds": ["IMPORTANT"]
                    }
                }
            ]
        }"#;

        let resp: FilterListResponse = serde_json::from_str(json).unwrap();
        let f = &resp.filter[0];
        assert_eq!(f.criteria.query.as_deref(), Some("is:important"));
        assert_eq!(
            f.criteria.negated_query.as_deref(),
            Some("from:spam@example.com")
        );
        assert_eq!(f.criteria.exclude_chats, Some(true));
        assert_eq!(f.criteria.size, Some(1048576));
        assert_eq!(f.criteria.size_comparison.as_deref(), Some("larger"));
    }
}
