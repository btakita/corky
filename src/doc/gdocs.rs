use anyhow::{Result, bail};
use std::path::Path;

use crate::filter::gmail_auth;

const DOCS_API: &str = "https://docs.googleapis.com/v1/documents";
const DOCS_EXPORT_URL: &str = "https://www.googleapis.com/drive/v3/files";

/// Extract a Google Docs document ID from a URL or raw ID.
pub fn parse_doc_id(input: &str) -> &str {
    // Handle URLs like https://docs.google.com/document/d/DOC_ID/edit
    if let Some(rest) = input.strip_prefix("https://docs.google.com/document/d/") {
        return rest.split('/').next().unwrap_or(rest);
    }
    // Already a bare ID
    input
}

/// Read a Google Doc and return its content as markdown (via HTML export).
pub fn read(doc: &str, output: Option<&Path>, account: Option<&str>) -> Result<()> {
    let doc_id = parse_doc_id(doc);
    let token =
        gmail_auth::get_access_token_for_user(Some("default"), gmail_auth::DOCS_SCOPE, account)?;

    // Export as HTML via Drive API (Docs API doesn't have a clean markdown export)
    let url = format!("{}/{}/export?mimeType=text/html", DOCS_EXPORT_URL, doc_id);

    eprintln!("Fetching Google Doc {}...", doc_id);
    let resp = api_get(&token, &url)?;
    let html = resp.into_string()?;

    // Convert HTML to plain text (strip tags)
    let markdown = strip_html_to_text(&html);

    if let Some(path) = output {
        std::fs::write(path, &markdown)?;
        eprintln!("Written to {}", path.display());
    } else {
        print!("{}", markdown);
    }

    Ok(())
}

/// Update a Google Doc from markdown content.
///
/// Strategy: clear the doc, then insert the new content as plain text.
/// (Full markdown→Docs structural conversion would require building
/// Docs API batch update requests for each element type.)
pub fn write(doc: &str, file: &Path, account: Option<&str>) -> Result<()> {
    let doc_id = parse_doc_id(doc);
    let token =
        gmail_auth::get_access_token_for_user(Some("default"), gmail_auth::DOCS_SCOPE, account)?;

    let content = std::fs::read_to_string(file)?;

    // Step 1: Get current document to find content length
    let url = format!("{}/{}", DOCS_API, doc_id);
    let resp = api_get(&token, &url)?;
    let doc_json: serde_json::Value = resp.into_json()?;

    let end_index = doc_json["body"]["content"]
        .as_array()
        .and_then(|arr| arr.last())
        .and_then(|elem| elem["endIndex"].as_i64())
        .unwrap_or(2); // Minimum is 2 (doc always has at least newline)

    // Step 2: Build batch update — delete all content, then insert new
    let mut requests = Vec::new();

    // Delete existing content (if more than just the trailing newline)
    if end_index > 2 {
        requests.push(serde_json::json!({
            "deleteContentRange": {
                "range": {
                    "startIndex": 1,
                    "endIndex": end_index - 1
                }
            }
        }));
    }

    // Insert new content at position 1
    requests.push(serde_json::json!({
        "insertText": {
            "location": { "index": 1 },
            "text": content
        }
    }));

    let batch_body = serde_json::json!({ "requests": requests });

    eprintln!("Updating Google Doc {}...", doc_id);
    let batch_url = format!("{}/{}:batchUpdate", DOCS_API, doc_id);
    let resp = ureq::post(&batch_url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&batch_body.to_string());

    match resp {
        Ok(_) => {
            eprintln!("Updated successfully.");
            Ok(())
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Docs API error (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("Docs API request failed: {}", e),
    }
}

/// Basic HTML tag stripping. Converts <br>, <p>, <div> to newlines.
fn strip_html_to_text(html: &str) -> String {
    let mut text = html.to_string();
    // Block-level elements → newlines
    for tag in &[
        "<br>", "<br/>", "<br />", "</p>", "</div>", "</li>", "</tr>", "</h1>", "</h2>", "</h3>",
        "</h4>", "</h5>", "</h6>",
    ] {
        text = text.replace(tag, "\n");
    }
    // List items
    text = text.replace("<li>", "- ");
    // Strip remaining tags
    let mut result = String::new();
    let mut in_tag = false;
    for ch in text.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
    }
    // Decode common HTML entities
    result = result.replace("&amp;", "&");
    result = result.replace("&lt;", "<");
    result = result.replace("&gt;", ">");
    result = result.replace("&quot;", "\"");
    result = result.replace("&#39;", "'");
    result = result.replace("&nbsp;", " ");
    // Collapse multiple blank lines
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result.trim().to_string()
}

fn api_get(token: &str, url: &str) -> Result<ureq::Response> {
    match ureq::get(url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(401, _)) => {
            bail!("Google API: unauthorized (401). Token may be expired.");
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Google API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}
