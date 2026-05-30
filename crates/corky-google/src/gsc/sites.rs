//! Search Console `sites.list` — list verified properties.

use anyhow::{Result, bail};
use serde::Deserialize;

const SITES_URL: &str = "https://searchconsole.googleapis.com/webmasters/v3/sites";

#[derive(Debug, Clone, Deserialize)]
pub struct SiteEntry {
    #[serde(rename = "siteUrl")]
    pub site_url: String,
    #[serde(rename = "permissionLevel")]
    pub permission_level: String,
}

#[derive(Debug, Deserialize)]
struct SitesListResponse {
    #[serde(default, rename = "siteEntry")]
    site_entry: Vec<SiteEntry>,
}

pub fn list_sites(token: &str) -> Result<Vec<SiteEntry>> {
    let resp = match ureq::get(SITES_URL)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("gsc sites.list failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => return Err(e.into()),
    };
    let body: SitesListResponse = resp.into_json()?;
    Ok(body.site_entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sites_list_response() {
        let json = r#"{
            "siteEntry": [
                {"siteUrl": "sc-domain:example.com", "permissionLevel": "siteOwner"},
                {"siteUrl": "https://foo.test/", "permissionLevel": "siteFullUser"}
            ]
        }"#;
        let parsed: SitesListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.site_entry.len(), 2);
        assert_eq!(parsed.site_entry[0].site_url, "sc-domain:example.com");
        assert_eq!(parsed.site_entry[1].permission_level, "siteFullUser");
    }

    #[test]
    fn parses_empty_sites_list() {
        let json = r#"{}"#;
        let parsed: SitesListResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.site_entry.is_empty());
    }
}
