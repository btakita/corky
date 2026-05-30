//! Compare local .corky.toml filters against live Gmail filters.
//!
//! Read-only — never modifies remote state. Used by `corky filter check`
//! and optionally by `corky watch` for drift detection.

use anyhow::Result;

use super::gmail_auth;
use super::push::{FilterCreateRequest, convert_filters, fetch_existing_filters, fetch_label_map};
use crate::config::corky_config;

/// Compare local config filters against Gmail and report drift.
/// Returns true if filters are in sync, false if drift detected.
pub fn run(account: Option<&str>) -> Result<bool> {
    let config = corky_config::load_config(None)?;
    let gmail = config
        .gmail
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No [gmail] section in .corky.toml"))?;

    if gmail.filters.is_empty() {
        println!("No [[gmail.filters]] in .corky.toml — nothing to check.");
        return Ok(true);
    }

    let access_token = gmail_auth::get_access_token(account)?;
    let label_map = fetch_label_map(&access_token)?;
    let local_filters = convert_filters(&gmail.filters, &label_map)?;
    let remote_filters = fetch_existing_filters(&access_token)?;

    let local_sigs = normalize_local(&local_filters);
    let remote_sigs = normalize_remote(&remote_filters);

    let missing_remote: Vec<&str> = local_sigs
        .iter()
        .filter(|sig| !remote_sigs.contains(sig))
        .map(|s| s.as_str())
        .collect();

    let extra_remote: Vec<&str> = remote_sigs
        .iter()
        .filter(|sig| !local_sigs.contains(sig))
        .map(|s| s.as_str())
        .collect();

    if missing_remote.is_empty() && extra_remote.is_empty() {
        println!("Filters in sync ({} filter(s)).", local_sigs.len());
        return Ok(true);
    }

    if !missing_remote.is_empty() {
        eprintln!(
            "Warning: {} filter(s) in .corky.toml but NOT on Gmail:",
            missing_remote.len()
        );
        for sig in &missing_remote {
            eprintln!("  - {}", truncate(sig, 80));
        }
    }

    if !extra_remote.is_empty() {
        eprintln!(
            "Warning: {} filter(s) on Gmail but NOT in .corky.toml:",
            extra_remote.len()
        );
        for sig in &extra_remote {
            eprintln!("  - {}", truncate(sig, 80));
        }
    }

    eprintln!("Run `corky filter push` to sync.");
    Ok(false)
}

/// Non-interactive variant for watch mode — never opens a browser.
/// Returns true if in sync, false if drift, Err if auth is missing/expired.
pub fn run_noninteractive(account: Option<&str>) -> Result<bool> {
    let config = corky_config::load_config(None)?;
    let gmail = config
        .gmail
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No [gmail] section in .corky.toml"))?;

    if gmail.filters.is_empty() {
        return Ok(true);
    }

    let access_token = gmail_auth::get_access_token_noninteractive(account)?;
    let label_map = fetch_label_map(&access_token)?;
    let local_filters = convert_filters(&gmail.filters, &label_map)?;
    let remote_filters = fetch_existing_filters(&access_token)?;

    let local_sigs = normalize_local(&local_filters);
    let remote_sigs = normalize_remote(&remote_filters);

    let missing_remote: Vec<&str> = local_sigs
        .iter()
        .filter(|sig| !remote_sigs.contains(sig))
        .map(|s| s.as_str())
        .collect();

    let extra_remote: Vec<&str> = remote_sigs
        .iter()
        .filter(|sig| !local_sigs.contains(sig))
        .map(|s| s.as_str())
        .collect();

    if missing_remote.is_empty() && extra_remote.is_empty() {
        return Ok(true);
    }

    Ok(false)
}

/// Normalize local filters to comparable signature strings.
fn normalize_local(filters: &[FilterCreateRequest]) -> Vec<String> {
    filters
        .iter()
        .map(|f| {
            let mut parts = Vec::new();
            if let Some(ref from) = f.criteria.from {
                parts.push(format!("from:{}", from));
            }
            if let Some(ref to) = f.criteria.to {
                parts.push(format!("to:{}", to));
            }
            if let Some(ref query) = f.criteria.query {
                parts.push(format!("query:{}", query));
            }
            parts.sort();
            parts.join("|")
        })
        .collect()
}

/// Normalize remote filters to comparable signature strings.
fn normalize_remote(filters: &[super::push::ExistingFilter]) -> Vec<String> {
    filters
        .iter()
        .map(|f| {
            let mut parts = Vec::new();
            if let Some(from) = f.criteria.get("from").and_then(|v| v.as_str()) {
                parts.push(format!("from:{}", from));
            }
            if let Some(to) = f.criteria.get("to").and_then(|v| v.as_str()) {
                parts.push(format!("to:{}", to));
            }
            if let Some(query) = f.criteria.get("query").and_then(|v| v.as_str()) {
                parts.push(format!("query:{}", query));
            }
            parts.sort();
            parts.join("|")
        })
        .collect()
}

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
    use super::super::push::{ApiAction, ApiCriteria, ExistingFilter};
    use super::*;

    #[test]
    fn test_normalize_local_from() {
        let filters = vec![FilterCreateRequest {
            criteria: ApiCriteria {
                from: Some("a@b.com OR c@d.com".to_string()),
                to: None,
                query: None,
            },
            action: ApiAction {
                add_label_ids: vec![],
                remove_label_ids: vec![],
                forward: None,
            },
        }];
        let sigs = normalize_local(&filters);
        assert_eq!(sigs, vec!["from:a@b.com OR c@d.com"]);
    }

    #[test]
    fn test_normalize_remote_from() {
        let filters = vec![ExistingFilter {
            id: "abc".to_string(),
            criteria: serde_json::json!({"from": "a@b.com OR c@d.com"}),
            action: serde_json::json!({}),
        }];
        let sigs = normalize_remote(&filters);
        assert_eq!(sigs, vec!["from:a@b.com OR c@d.com"]);
    }

    #[test]
    fn test_normalize_query_filter() {
        let local = vec![FilterCreateRequest {
            criteria: ApiCriteria {
                from: None,
                to: None,
                query: Some("from:(x@y.com) OR to:(x@y.com)".to_string()),
            },
            action: ApiAction {
                add_label_ids: vec![],
                remove_label_ids: vec![],
                forward: None,
            },
        }];
        let remote = vec![ExistingFilter {
            id: "def".to_string(),
            criteria: serde_json::json!({"query": "from:(x@y.com) OR to:(x@y.com)"}),
            action: serde_json::json!({}),
        }];
        let local_sigs = normalize_local(&local);
        let remote_sigs = normalize_remote(&remote);
        assert_eq!(local_sigs, remote_sigs);
    }

    #[test]
    fn test_drift_detection() {
        let local = vec![FilterCreateRequest {
            criteria: ApiCriteria {
                from: Some("a@b.com".to_string()),
                to: None,
                query: None,
            },
            action: ApiAction {
                add_label_ids: vec![],
                remove_label_ids: vec![],
                forward: None,
            },
        }];
        let remote = vec![ExistingFilter {
            id: "xyz".to_string(),
            criteria: serde_json::json!({"from": "different@example.com"}),
            action: serde_json::json!({}),
        }];
        let local_sigs = normalize_local(&local);
        let remote_sigs = normalize_remote(&remote);
        // They should NOT match
        let missing: Vec<_> = local_sigs
            .iter()
            .filter(|s| !remote_sigs.contains(s))
            .collect();
        let extra: Vec<_> = remote_sigs
            .iter()
            .filter(|s| !local_sigs.contains(s))
            .collect();
        assert_eq!(missing.len(), 1);
        assert_eq!(extra.len(), 1);
    }
}
