use anyhow::{bail, Result};
use std::path::Path;

use crate::filter::gmail_auth;

const SHEETS_API: &str = "https://sheets.googleapis.com/v4/spreadsheets";

/// Extract a Google Sheets spreadsheet ID from a URL or raw ID.
pub fn parse_sheet_id(input: &str) -> &str {
    // Handle URLs like https://docs.google.com/spreadsheets/d/SHEET_ID/edit
    if let Some(rest) = input.strip_prefix("https://docs.google.com/spreadsheets/d/") {
        return rest.split('/').next().unwrap_or(rest);
    }
    input
}

/// Read a Google Sheet range and output as markdown table or CSV.
pub fn read(sheet: &str, range: Option<&str>, format: &str, output: Option<&Path>) -> Result<()> {
    let sheet_id = parse_sheet_id(sheet);
    let token = gmail_auth::get_access_token_with_scope(
        Some("default"),
        gmail_auth::SHEETS_READONLY_SCOPE,
    )?;

    // Build URL with optional range
    let url = if let Some(range) = range {
        format!(
            "{}/{}/values/{}",
            SHEETS_API,
            sheet_id,
            encode_range(range)
        )
    } else {
        // Get sheet metadata first to find the first sheet name
        let meta_url = format!("{}/{}?fields=sheets.properties.title", SHEETS_API, sheet_id);
        let meta_resp = api_get(&token, &meta_url)?;
        let meta: serde_json::Value = meta_resp.into_json()?;
        let first_sheet = meta["sheets"][0]["properties"]["title"]
            .as_str()
            .unwrap_or("Sheet1");
        format!("{}/{}/values/{}", SHEETS_API, sheet_id, first_sheet)
    };

    eprintln!("Fetching sheet data...");
    let resp = api_get(&token, &url)?;
    let data: serde_json::Value = resp.into_json()?;

    let rows: Vec<Vec<String>> = data["values"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|row| {
                    row.as_array()
                        .map(|cells| {
                            cells
                                .iter()
                                .map(|c| c.as_str().unwrap_or("").to_string())
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();

    if rows.is_empty() {
        eprintln!("No data found.");
        return Ok(());
    }

    let result = match format {
        "csv" => format_csv(&rows),
        _ => format_markdown_table(&rows),
    };

    if let Some(path) = output {
        std::fs::write(path, &result)?;
        eprintln!("Written to {}", path.display());
    } else {
        print!("{}", result);
    }

    Ok(())
}

fn format_markdown_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    // Find max columns
    let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);

    // Column widths
    let mut widths = vec![3usize; max_cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < max_cols {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    let mut out = String::new();

    // Header row
    let header = &rows[0];
    out.push('|');
    for (i, w) in widths.iter().enumerate().take(max_cols) {
        let cell = header.get(i).map(|s| s.as_str()).unwrap_or("");
        out.push_str(&format!(" {:<width$} |", cell, width = w));
    }
    out.push('\n');

    // Separator
    out.push('|');
    for w in &widths {
        out.push_str(&format!("-{}-|", "-".repeat(*w)));
    }
    out.push('\n');

    // Data rows
    for row in &rows[1..] {
        out.push('|');
        for (i, w) in widths.iter().enumerate().take(max_cols) {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            out.push_str(&format!(" {:<width$} |", cell, width = w));
        }
        out.push('\n');
    }

    out
}

fn format_csv(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|cell| {
                    if cell.contains(',') || cell.contains('"') || cell.contains('\n') {
                        format!("\"{}\"", cell.replace('"', "\"\""))
                    } else {
                        cell.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// URL-encode a sheet range (e.g., "Sheet1!A1:D10" → "Sheet1%21A1%3AD10").
fn encode_range(range: &str) -> String {
    range
        .replace('!', "%21")
        .replace(':', "%3A")
        .replace(' ', "%20")
}

fn api_get(token: &str, url: &str) -> Result<ureq::Response> {
    match ureq::get(url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(401, _)) => {
            bail!("Sheets API: unauthorized (401). Token may be expired.");
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Sheets API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}
