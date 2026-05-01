//! Push local Gmail filter config to Gmail via the Settings API.
//!
//! Replaces all existing Gmail filters with those defined in .corky.toml.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;

use super::gmail_auth;
use crate::config::corky_config::{self, GmailFilter};

const GMAIL_API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

// --- API response types (pub(crate) for reuse in check.rs) ---

#[derive(Debug, serde::Deserialize)]
pub(crate) struct FilterListResponse {
    #[serde(default)]
    filter: Vec<ExistingFilter>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ExistingFilter {
    pub id: String,
    #[serde(default)]
    pub criteria: serde_json::Value,
    #[serde(default)]
    #[allow(dead_code)]
    pub action: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
struct LabelListResponse {
    #[serde(default)]
    labels: Vec<GmailLabelEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct GmailLabelEntry {
    id: String,
    name: String,
}

// --- API request types (pub(crate) for reuse in check.rs) ---

#[derive(Debug, serde::Serialize)]
pub(crate) struct FilterCreateRequest {
    pub(crate) criteria: ApiCriteria,
    pub(crate) action: ApiAction,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiCriteria {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) query: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiAction {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) add_label_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) remove_label_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) forward: Option<String>,
}

// --- Main entry point ---

pub fn run(account: Option<&str>, dry_run: bool) -> Result<()> {
    // 1. Load config
    let config = corky_config::load_config(None)?;
    let gmail = config
        .gmail
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No [gmail] section in .corky.toml"))?;

    if gmail.filters.is_empty() {
        bail!("No [[gmail.filters]] entries in .corky.toml — nothing to push.");
    }

    // 2. Authenticate
    let access_token = gmail_auth::get_access_token(account)?;

    // 3. Fetch label name→ID mapping
    let mut label_map = fetch_label_map(&access_token)?;

    // 3b. Create any missing labels referenced by filters
    ensure_labels(&gmail.filters, &mut label_map, &access_token)?;

    // 4. Convert config filters to API format
    let api_filters = convert_filters(&gmail.filters, &label_map)?;

    // 5. Fetch existing filters
    let existing = fetch_existing_filters(&access_token)?;

    if dry_run {
        println!("DRY RUN — no changes will be made.\n");
        println!("Existing filters: {}", existing.len());
        for (i, f) in existing.iter().enumerate() {
            print!("  [{}] id={}", i + 1, f.id);
            if let Some(from) = f.criteria.get("from").and_then(|v| v.as_str()) {
                print!("  from: {}", truncate(from, 60));
            }
            if let Some(to) = f.criteria.get("to").and_then(|v| v.as_str()) {
                print!("  to: {}", truncate(to, 60));
            }
            println!();
        }
        println!("\nWould delete ALL {} existing filter(s).", existing.len());
        println!("Would create {} new filter(s):\n", api_filters.len());
        for (i, f) in api_filters.iter().enumerate() {
            println!("  [{}]", i + 1);
            if let Some(ref from) = f.criteria.from {
                println!("      from: {}", truncate(from, 70));
            }
            if let Some(ref to) = f.criteria.to {
                println!("      to: {}", truncate(to, 70));
            }
            if let Some(ref query) = f.criteria.query {
                println!("      query: {}", truncate(query, 70));
            }
            if !f.action.add_label_ids.is_empty() {
                println!("      add_labels: {}", f.action.add_label_ids.join(", "));
            }
            if !f.action.remove_label_ids.is_empty() {
                println!(
                    "      remove_labels: {}",
                    f.action.remove_label_ids.join(", ")
                );
            }
            if let Some(ref fwd) = f.action.forward {
                println!("      forward: {}", fwd);
            }
        }
        return Ok(());
    }

    // 6. Delete all existing filters
    if !existing.is_empty() {
        println!("Deleting {} existing filter(s)...", existing.len());
        for f in &existing {
            delete_filter(&access_token, &f.id)?;
        }
        println!("  Done.");
    }

    // 7. Create new filters
    println!("Creating {} filter(s)...", api_filters.len());
    for (i, f) in api_filters.iter().enumerate() {
        let hint = f
            .criteria
            .from
            .as_deref()
            .or(f.criteria.to.as_deref())
            .or(f.criteria.query.as_deref())
            .unwrap_or("(no criteria)");
        create_filter(&access_token, f)?;
        println!("  [{}] {}", i + 1, truncate(hint, 60));
    }

    println!("\nDone. {} filter(s) pushed to Gmail.", api_filters.len());
    Ok(())
}

// --- Label resolution ---

pub(crate) fn fetch_label_map(token: &str) -> Result<HashMap<String, String>> {
    let resp = api_get(token, &format!("{}/labels", GMAIL_API))?;
    let body: LabelListResponse = resp
        .into_json()
        .context("Failed to parse labels response")?;

    let mut map = HashMap::new();
    for label in body.labels {
        map.insert(label.name.to_lowercase(), label.id);
    }
    Ok(map)
}

/// Ensure all user labels referenced by filters exist in Gmail.
/// Creates any missing labels via the API and updates the label map.
fn ensure_labels(
    filters: &[GmailFilter],
    label_map: &mut HashMap<String, String>,
    token: &str,
) -> Result<()> {
    for filt in filters {
        if let Some(ref label_name) = filt.label {
            // Skip system labels
            let upper = label_name.to_uppercase();
            if matches!(
                upper.as_str(),
                "INBOX" | "STARRED" | "IMPORTANT" | "SENT" | "DRAFT" | "SPAM" | "TRASH" | "UNREAD"
            ) || upper.starts_with("CATEGORY_")
            {
                continue;
            }

            // Create if missing
            if let std::collections::hash_map::Entry::Vacant(e) =
                label_map.entry(label_name.to_lowercase())
            {
                println!("Creating missing Gmail label '{}'...", label_name);
                let id = create_gmail_label(token, label_name)?;
                e.insert(id);
            }
        }
    }
    Ok(())
}

fn resolve_label_id(name: &str, label_map: &HashMap<String, String>) -> Result<String> {
    // System labels (well-known IDs)
    let upper = name.to_uppercase();
    match upper.as_str() {
        "INBOX" | "STARRED" | "IMPORTANT" | "SENT" | "DRAFT" | "SPAM" | "TRASH" | "UNREAD" => {
            return Ok(upper);
        }
        _ => {}
    }
    if upper.starts_with("CATEGORY_") {
        return Ok(upper);
    }

    // User labels (case-insensitive lookup)
    if let Some(id) = label_map.get(&name.to_lowercase()) {
        return Ok(id.clone());
    }

    bail!(
        "Gmail label '{}' not found.\n\
         Check that the label exists in Gmail, or create it first.",
        name
    )
}

// --- Filter conversion ---

pub(crate) fn convert_filters(
    filters: &[GmailFilter],
    label_map: &HashMap<String, String>,
) -> Result<Vec<FilterCreateRequest>> {
    let mut result = Vec::new();
    for (i, filt) in filters.iter().enumerate() {
        result.push(convert_one(filt, label_map).with_context(|| {
            format!(
                "Filter #{} (label={:?})",
                i + 1,
                filt.label.as_deref().unwrap_or("none")
            )
        })?);
    }
    Ok(result)
}

fn convert_one(
    filt: &GmailFilter,
    label_map: &HashMap<String, String>,
) -> Result<FilterCreateRequest> {
    let addr_str = filt.addresses.join(" OR ");
    let match_fields = if filt.match_fields.is_empty() {
        vec!["from".to_string()]
    } else {
        filt.match_fields.clone()
    };

    // When multiple match fields are specified, use `query` with OR logic
    // to avoid Gmail's AND behavior on separate from/to criteria.
    let criteria = if match_fields.len() > 1 {
        let parts: Vec<String> = match_fields
            .iter()
            .map(|field| format!("{}:({})", field, addr_str))
            .collect();
        ApiCriteria {
            from: None,
            to: None,
            query: Some(parts.join(" OR ")),
        }
    } else {
        ApiCriteria {
            from: if match_fields.contains(&"from".to_string()) {
                Some(addr_str.clone())
            } else {
                None
            },
            to: if match_fields.contains(&"to".to_string()) {
                Some(addr_str)
            } else {
                None
            },
            query: None,
        }
    };

    let mut add_label_ids = Vec::new();
    let mut remove_label_ids = Vec::new();

    // Resolve user label
    if let Some(ref label_name) = filt.label {
        let label_id = resolve_label_id(label_name, label_map)?;
        add_label_ids.push(label_id);
    }

    // System label actions
    if filt.star {
        add_label_ids.push("STARRED".to_string());
    }
    if filt.always_important {
        add_label_ids.push("IMPORTANT".to_string());
    }
    if filt.never_spam {
        remove_label_ids.push("SPAM".to_string());
    }

    let action = ApiAction {
        add_label_ids,
        remove_label_ids,
        forward: filt.forward_to.clone(),
    };

    Ok(FilterCreateRequest { criteria, action })
}

// --- API calls ---

pub(crate) fn fetch_existing_filters(token: &str) -> Result<Vec<ExistingFilter>> {
    let resp = api_get(token, &format!("{}/settings/filters", GMAIL_API))?;
    let body: FilterListResponse = resp
        .into_json()
        .context("Failed to parse filters response")?;
    Ok(body.filter)
}

fn delete_filter(token: &str, filter_id: &str) -> Result<()> {
    let url = format!("{}/settings/filters/{}", GMAIL_API, filter_id);
    match ureq::delete(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!(
                "Failed to delete filter {} (HTTP {}): {}",
                filter_id,
                status,
                err_body
            );
        }
        Err(e) => Err(e.into()),
    }
}

/// Create a Gmail label via the API and return its ID.
fn create_gmail_label(token: &str, name: &str) -> Result<String> {
    let url = format!("{}/labels", GMAIL_API);
    let body = serde_json::json!({ "name": name });
    match ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(body)
    {
        Ok(resp) => {
            let entry: GmailLabelEntry = resp
                .into_json()
                .context("Failed to parse label creation response")?;
            Ok(entry.id)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!(
                "Failed to create label '{}' (HTTP {}): {}",
                name,
                status,
                err_body
            );
        }
        Err(e) => Err(e.into()),
    }
}

fn create_filter(token: &str, filter: &FilterCreateRequest) -> Result<()> {
    let url = format!("{}/settings/filters", GMAIL_API);
    let json_value = serde_json::to_value(filter)?;
    match ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(json_value)
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Failed to create filter (HTTP {}): {}", status, err_body);
        }
        Err(e) => Err(e.into()),
    }
}

fn api_get(token: &str, url: &str) -> Result<ureq::Response> {
    match ureq::get(url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(401, _)) => {
            bail!(
                "Gmail API returned 401 Unauthorized.\n\
                 Try re-authenticating with: corky filter auth"
            );
        }
        Err(ureq::Error::Status(403, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            if err_body.contains("insufficientPermissions") {
                bail!(
                    "Gmail API returned 403 Insufficient Permissions.\n\
                     The token may be missing required scopes. Re-authenticate with:\n\
                     corky filter auth"
                );
            }
            bail!("Gmail API error (HTTP 403): {}", err_body);
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Gmail API error (HTTP {}): {}", status, err_body);
        }
        Err(e) => Err(e.into()),
    }
}

// --- Helpers ---

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{}...", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_label_map() -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("for-lucas".to_string(), "Label_5".to_string());
        map.insert("correspondence".to_string(), "Label_12".to_string());
        map
    }

    #[test]
    fn test_convert_from_only_filter() {
        let filt = GmailFilter {
            label: Some("for-lucas".to_string()),
            match_fields: vec!["from".to_string()],
            addresses: vec!["a@b.com".to_string(), "c@d.com".to_string()],
            forward_to: Some("dev@example.com".to_string()),
            star: true,
            never_spam: true,
            always_important: true,
        };
        let req = convert_one(&filt, &sample_label_map()).unwrap();
        assert_eq!(req.criteria.from.as_deref(), Some("a@b.com OR c@d.com"));
        assert!(req.criteria.to.is_none());
        assert!(req.action.add_label_ids.contains(&"Label_5".to_string()));
        assert!(req.action.add_label_ids.contains(&"STARRED".to_string()));
        assert!(req.action.add_label_ids.contains(&"IMPORTANT".to_string()));
        assert!(req.action.remove_label_ids.contains(&"SPAM".to_string()));
        assert_eq!(req.action.forward.as_deref(), Some("dev@example.com"));
    }

    #[test]
    fn test_convert_from_and_to_filter_uses_query() {
        let filt = GmailFilter {
            label: Some("correspondence".to_string()),
            match_fields: vec!["from".to_string(), "to".to_string()],
            addresses: vec!["x@y.com".to_string()],
            forward_to: None,
            star: false,
            never_spam: false,
            always_important: true,
        };
        let req = convert_one(&filt, &sample_label_map()).unwrap();
        // Multiple match fields use query with OR instead of separate from/to (AND)
        assert!(req.criteria.from.is_none());
        assert!(req.criteria.to.is_none());
        assert_eq!(
            req.criteria.query.as_deref(),
            Some("from:(x@y.com) OR to:(x@y.com)")
        );
        assert!(req.action.add_label_ids.contains(&"Label_12".to_string()));
        assert!(req.action.add_label_ids.contains(&"IMPORTANT".to_string()));
        assert!(req.action.remove_label_ids.is_empty());
        assert!(req.action.forward.is_none());
    }

    #[test]
    fn test_convert_default_match_is_from() {
        let filt = GmailFilter {
            label: None,
            match_fields: vec![],
            addresses: vec!["a@b.com".to_string()],
            forward_to: None,
            star: false,
            never_spam: false,
            always_important: false,
        };
        let req = convert_one(&filt, &sample_label_map()).unwrap();
        assert!(req.criteria.from.is_some());
        assert!(req.criteria.to.is_none());
    }

    #[test]
    fn test_resolve_system_labels() {
        let map = HashMap::new();
        assert_eq!(resolve_label_id("STARRED", &map).unwrap(), "STARRED");
        assert_eq!(resolve_label_id("IMPORTANT", &map).unwrap(), "IMPORTANT");
        assert_eq!(resolve_label_id("SPAM", &map).unwrap(), "SPAM");
        assert_eq!(resolve_label_id("INBOX", &map).unwrap(), "INBOX");
        assert_eq!(resolve_label_id("UNREAD", &map).unwrap(), "UNREAD");
        assert_eq!(
            resolve_label_id("CATEGORY_SOCIAL", &map).unwrap(),
            "CATEGORY_SOCIAL"
        );
    }

    #[test]
    fn test_resolve_user_label() {
        let map = sample_label_map();
        assert_eq!(resolve_label_id("for-lucas", &map).unwrap(), "Label_5");
        // Case insensitive
        assert_eq!(resolve_label_id("For-Lucas", &map).unwrap(), "Label_5");
    }

    #[test]
    fn test_resolve_unknown_label() {
        let map = HashMap::new();
        assert!(resolve_label_id("nonexistent", &map).is_err());
    }

    #[test]
    fn test_deserialize_label_list() {
        let json = r#"{
            "labels": [
                {"id": "INBOX", "name": "INBOX"},
                {"id": "Label_5", "name": "for-lucas"},
                {"id": "Label_12", "name": "correspondence"}
            ]
        }"#;
        let resp: LabelListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.labels.len(), 3);
        assert_eq!(resp.labels[1].name, "for-lucas");
        assert_eq!(resp.labels[1].id, "Label_5");
    }

    #[test]
    fn test_deserialize_filter_list() {
        let json = r#"{
            "filter": [
                {
                    "id": "abc123",
                    "criteria": {"from": "test@example.com"},
                    "action": {"addLabelIds": ["STARRED"]}
                }
            ]
        }"#;
        let resp: FilterListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.filter.len(), 1);
        assert_eq!(resp.filter[0].id, "abc123");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn test_api_action_serialization() {
        let req = FilterCreateRequest {
            criteria: ApiCriteria {
                from: Some("a@b.com OR c@d.com".to_string()),
                to: None,
                query: None,
            },
            action: ApiAction {
                add_label_ids: vec!["Label_5".to_string(), "STARRED".to_string()],
                remove_label_ids: vec!["SPAM".to_string()],
                forward: Some("dev@example.com".to_string()),
            },
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["criteria"]["from"], "a@b.com OR c@d.com");
        assert!(json["criteria"].get("to").is_none());
        assert!(json["criteria"].get("query").is_none());
        assert_eq!(json["action"]["addLabelIds"][0], "Label_5");
        assert_eq!(json["action"]["removeLabelIds"][0], "SPAM");
        assert_eq!(json["action"]["forward"], "dev@example.com");
    }

    #[test]
    fn test_api_action_empty_fields_omitted() {
        let req = FilterCreateRequest {
            criteria: ApiCriteria {
                from: Some("a@b.com".to_string()),
                to: None,
                query: None,
            },
            action: ApiAction {
                add_label_ids: vec!["Label_5".to_string()],
                remove_label_ids: vec![],
                forward: None,
            },
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json["action"].get("removeLabelIds").is_none());
        assert!(json["action"].get("forward").is_none());
    }
}
