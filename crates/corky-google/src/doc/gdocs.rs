use anyhow::{Result, bail};
use std::path::Path;

use crate::filter::gmail_auth;

const DOCS_API: &str = "https://docs.googleapis.com/v1/documents";

/// Extract a Google Docs document ID from a URL or raw ID.
pub fn parse_doc_id(input: &str) -> &str {
    // Handle URLs like https://docs.google.com/document/d/DOC_ID/edit
    if let Some(rest) = input.strip_prefix("https://docs.google.com/document/d/") {
        return rest.split('/').next().unwrap_or(rest);
    }
    // Already a bare ID
    input
}

/// Read a Google Doc and return its text content as markdown-ish plain text.
pub fn read(doc: &str, output: Option<&Path>, account: Option<&str>) -> Result<()> {
    let doc_id = parse_doc_id(doc);
    let token =
        gmail_auth::get_access_token_for_user(Some("default"), gmail_auth::DOCS_SCOPE, account)?;

    eprintln!("Fetching Google Doc {}...", doc_id);
    let url = format!("{}/{}", DOCS_API, doc_id);
    let resp = api_get(&token, &url)?;
    let doc_json: serde_json::Value = resp.into_json()?;
    let markdown = document_to_text(&doc_json);

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

fn document_to_text(document: &serde_json::Value) -> String {
    let mut out = String::new();
    if let Some(content) = document["body"]["content"].as_array() {
        append_structural_elements(content, &mut out);
    }
    out.trim().to_string()
}

fn append_structural_elements(elements: &[serde_json::Value], out: &mut String) {
    for element in elements {
        if let Some(paragraph_elements) = element["paragraph"]["elements"].as_array() {
            for paragraph_element in paragraph_elements {
                if let Some(text) = paragraph_element["textRun"]["content"].as_str() {
                    out.push_str(text);
                }
            }
        }

        if let Some(rows) = element["table"]["tableRows"].as_array() {
            for row in rows {
                if let Some(cells) = row["tableCells"].as_array() {
                    let mut rendered_cells = Vec::new();
                    for cell in cells {
                        let mut cell_text = String::new();
                        if let Some(cell_content) = cell["content"].as_array() {
                            append_structural_elements(cell_content, &mut cell_text);
                        }
                        rendered_cells.push(cell_text.trim().replace('\n', " "));
                    }
                    out.push_str(&rendered_cells.join("\t"));
                    out.push('\n');
                }
            }
        }
    }
}

fn api_get(token: &str, url: &str) -> Result<ureq::Response> {
    match ureq::get(url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(401, _)) => {
            let _ = gmail_auth::clear_access_token(token);
            bail!("Google API: unauthorized (401). Cleared the cached token; it may be expired.");
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Google API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_doc_id_accepts_raw_id() {
        assert_eq!(parse_doc_id("doc-id"), "doc-id");
    }

    #[test]
    fn parse_doc_id_extracts_url_id() {
        assert_eq!(
            parse_doc_id("https://docs.google.com/document/d/doc-id/edit"),
            "doc-id"
        );
    }

    #[test]
    fn document_to_text_extracts_paragraph_text() {
        let doc = serde_json::json!({
            "body": {
                "content": [
                    {
                        "paragraph": {
                            "elements": [
                                { "textRun": { "content": "Hello " } },
                                { "textRun": { "content": "Docs\n" } }
                            ]
                        }
                    },
                    {
                        "paragraph": {
                            "elements": [
                                { "textRun": { "content": "Second paragraph\n" } }
                            ]
                        }
                    }
                ]
            }
        });

        assert_eq!(document_to_text(&doc), "Hello Docs\nSecond paragraph");
    }

    #[test]
    fn document_to_text_extracts_table_cell_text() {
        let doc = serde_json::json!({
            "body": {
                "content": [
                    {
                        "table": {
                            "tableRows": [
                                {
                                    "tableCells": [
                                        {
                                            "content": [
                                                {
                                                    "paragraph": {
                                                        "elements": [
                                                            { "textRun": { "content": "Name\n" } }
                                                        ]
                                                    }
                                                }
                                            ]
                                        },
                                        {
                                            "content": [
                                                {
                                                    "paragraph": {
                                                        "elements": [
                                                            { "textRun": { "content": "Score\n" } }
                                                        ]
                                                    }
                                                }
                                            ]
                                        }
                                    ]
                                }
                            ]
                        }
                    }
                ]
            }
        });

        assert_eq!(document_to_text(&doc), "Name\tScore");
    }
}
