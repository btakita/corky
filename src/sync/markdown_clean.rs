//! Post-processing for HTML→markdown conversion output.
//!
//! Strips CSS artifacts, tracking pixels, and other email noise that
//! `htmd` passes through from the HTML source.

use once_cell::sync::Lazy;
use regex::Regex;

/// Regex matching `@media` blocks (may span multiple lines).
static MEDIA_QUERY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)@media[^{]*\{(?:[^{}]*\{[^}]*\})*[^}]*\}").unwrap());

/// Regex matching tracking pixel images: `![...](url)` where url contains
/// known tracking paths or the image has 1x1 dimensions.
static TRACKING_IMG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"!\[[^\]]*\]\([^)]*(?:/wf/open|/analytics/open|/o\.gif|/t\.gif|/track/open|width=['"]*1['"]*|height=['"]*1['"]*)[^)]*\)"#,
    )
    .unwrap()
});

/// Regex to extract domain from a markdown image URL: `![...](http(s)://domain/...)`.
static IMG_URL_DOMAIN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"!\[[^\]]*\]\(https?://([^/)\s]+)").unwrap());

/// Regex matching inline HTML style attributes.
static STYLE_ATTR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\s*style\s*=\s*"[^"]*""#).unwrap());

/// Regex matching lines that are only whitespace or empty after cleanup.
static EXCESS_BLANK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{3,}").unwrap());

/// Clean markdown output from `htmd` conversion.
///
/// Returns `(cleaned_markdown, tracking_domains)`.
pub fn clean_markdown(md: &str) -> (String, Vec<String>) {
    let tracking_domains = extract_tracking_domains(md);

    let mut cleaned = md.to_string();

    // Strip @media blocks
    cleaned = MEDIA_QUERY_RE.replace_all(&cleaned, "").to_string();

    // Strip tracking pixel images
    cleaned = TRACKING_IMG_RE.replace_all(&cleaned, "").to_string();

    // Strip inline style attributes
    cleaned = STYLE_ATTR_RE.replace_all(&cleaned, "").to_string();

    // Collapse excessive blank lines
    cleaned = EXCESS_BLANK_RE.replace_all(&cleaned, "\n\n").to_string();

    // Trim leading/trailing whitespace
    cleaned = cleaned.trim().to_string();

    (cleaned, tracking_domains)
}

/// Extract unique tracking pixel domains from markdown.
fn extract_tracking_domains(md: &str) -> Vec<String> {
    let mut domains: Vec<String> = Vec::new();

    for cap in TRACKING_IMG_RE.find_iter(md) {
        if let Some(domain_cap) = IMG_URL_DOMAIN_RE.captures(cap.as_str()) {
            let domain = domain_cap[1].to_string();
            if !domains.contains(&domain) {
                domains.push(domain);
            }
        }
    }

    domains
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_media_queries() {
        let md = "Hello\n\n@media only screen and (max-width: 720px) { table { font-size: 14px; } }\n\nWorld";
        let (cleaned, _) = clean_markdown(md);
        assert!(!cleaned.contains("@media"), "should strip @media block");
        assert!(cleaned.contains("Hello"));
        assert!(cleaned.contains("World"));
    }

    #[test]
    fn strips_tracking_pixels() {
        let md =
            "Content here\n\n![](http://tracker.example.com/wf/open?id=abc123)\n\nMore content";
        let (cleaned, domains) = clean_markdown(md);
        assert!(!cleaned.contains("wf/open"), "should strip tracking pixel");
        assert_eq!(domains, vec!["tracker.example.com"]);
    }

    #[test]
    fn strips_1x1_images() {
        let md = "Text\n\n![](http://spy.example.com/img?width='1' height='1')\n\nEnd";
        let (cleaned, domains) = clean_markdown(md);
        assert!(
            !cleaned.contains("spy.example.com"),
            "should strip 1x1 image"
        );
        assert_eq!(domains, vec!["spy.example.com"]);
    }

    #[test]
    fn strips_inline_styles() {
        let md = r#"<div style="color: red; font-size: 14px">Hello</div>"#;
        let (cleaned, _) = clean_markdown(md);
        assert!(!cleaned.contains("style="), "should strip inline styles");
    }

    #[test]
    fn preserves_normal_images() {
        let md = "![Logo](http://example.com/logo.png)\n\nContent";
        let (cleaned, domains) = clean_markdown(md);
        assert!(cleaned.contains("![Logo](http://example.com/logo.png)"));
        assert!(domains.is_empty());
    }

    #[test]
    fn collapses_excess_blank_lines() {
        let md = "A\n\n\n\n\nB";
        let (cleaned, _) = clean_markdown(md);
        assert_eq!(cleaned, "A\n\nB");
    }

    #[test]
    fn deduplicates_tracking_domains() {
        let md = "![](http://t.example.com/wf/open?a=1)\n![](http://t.example.com/wf/open?b=2)";
        let (_, domains) = clean_markdown(md);
        assert_eq!(domains, vec!["t.example.com"]);
    }

    #[test]
    fn strips_analytics_open_tracking() {
        let md = "Body\n\n![](https://manage.turing.com/analytics/open/abc123)\n\nEnd";
        let (cleaned, domains) = clean_markdown(md);
        assert!(!cleaned.contains("analytics/open"));
        assert_eq!(domains, vec!["manage.turing.com"]);
    }

    #[test]
    fn strips_nested_media_queries() {
        let md = "Start\n\n@media only screen and (max-width: 720px) { table[class=body] h1 { font-size: 22px !important; } table[class=body] .content { padding: 10px !important; } }\n\nEnd";
        let (cleaned, _) = clean_markdown(md);
        assert!(!cleaned.contains("@media"));
        assert!(cleaned.contains("Start"));
        assert!(cleaned.contains("End"));
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        let md = "Hello world.\n\nThis is plain text with no HTML artifacts.";
        let (cleaned, domains) = clean_markdown(md);
        assert_eq!(cleaned, md);
        assert!(domains.is_empty());
    }

    #[test]
    fn multiple_tracking_domains_detected() {
        let md = "![](http://a.sendgrid.net/wf/open?x=1)\n![](http://b.outlier.ai/wf/open?y=2)";
        let (_, domains) = clean_markdown(md);
        assert_eq!(domains.len(), 2);
        assert!(domains.contains(&"a.sendgrid.net".to_string()));
        assert!(domains.contains(&"b.outlier.ai".to_string()));
    }
}
