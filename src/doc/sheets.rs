use anyhow::{Result, bail};
use std::path::Path;

use crate::filter::gmail_auth;

const SHEETS_WRITE_API: &str = "https://sheets.googleapis.com/v4/spreadsheets";

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
pub fn read(
    sheet: &str,
    range: Option<&str>,
    format: &str,
    output: Option<&Path>,
    account: Option<&str>,
) -> Result<()> {
    let sheet_id = parse_sheet_id(sheet);
    let token = gmail_auth::get_access_token_for_user(
        Some("default"),
        gmail_auth::SHEETS_READONLY_SCOPE,
        account,
    )?;

    // Build URL with optional range
    let url = if let Some(range) = range {
        format!("{}/{}/values/{}", SHEETS_API, sheet_id, encode_range(range))
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

/// Write a CSV file to a Google Sheet range.
///
/// Requires `SHEETS_SCOPE` (read/write), not readonly.
pub fn write(sheet: &str, range: &str, file: &Path, account: Option<&str>) -> Result<()> {
    let sheet_id = parse_sheet_id(sheet);
    let token =
        gmail_auth::get_access_token_for_user(Some("default"), gmail_auth::SHEETS_SCOPE, account)?;

    let csv_content = std::fs::read_to_string(file)?;
    let values: Vec<Vec<String>> = csv_content.lines().map(parse_csv_line).collect();

    if values.is_empty() {
        eprintln!("No data in CSV file.");
        return Ok(());
    }

    let json_values: Vec<serde_json::Value> = values
        .into_iter()
        .map(|row| {
            serde_json::Value::Array(row.into_iter().map(serde_json::Value::String).collect())
        })
        .collect();

    let body = serde_json::json!({
        "values": json_values
    });

    let url = format!(
        "{}/{}/values/{}?valueInputOption=USER_ENTERED",
        SHEETS_WRITE_API,
        sheet_id,
        encode_range(range)
    );

    eprintln!("Writing {} rows to {}...", json_values.len(), range);

    let resp = ureq::put(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(&body);

    match resp {
        Ok(r) => {
            let result: serde_json::Value = r.into_json()?;
            let updated = result["updatedCells"].as_u64().unwrap_or(0);
            println!("Updated {} cells.", updated);
            Ok(())
        }
        Err(ureq::Error::Status(401, _)) => {
            bail!("Sheets API: unauthorized (401). Re-run `corky filter auth`.")
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Sheets API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Parse a single CSV line, handling quoted fields.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            other => current.push(other),
        }
    }
    fields.push(current);
    fields
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sheet_id_raw() {
        assert_eq!(parse_sheet_id("abc123"), "abc123");
    }

    #[test]
    fn test_parse_sheet_id_url() {
        assert_eq!(
            parse_sheet_id("https://docs.google.com/spreadsheets/d/abc123/edit"),
            "abc123"
        );
    }

    #[test]
    fn test_encode_range() {
        assert_eq!(encode_range("Sheet1!A1:D10"), "Sheet1%21A1%3AD10");
        assert_eq!(encode_range("A1"), "A1");
    }

    #[test]
    fn test_parse_csv_line_simple() {
        assert_eq!(parse_csv_line("a,b,c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_csv_line_quoted() {
        assert_eq!(
            parse_csv_line(r#""hello, world",b"#),
            vec!["hello, world", "b"]
        );
    }

    #[test]
    fn test_parse_csv_line_escaped_quotes() {
        assert_eq!(parse_csv_line(r#""say ""hi""",b"#), vec!["say \"hi\"", "b"]);
    }

    #[test]
    fn test_parse_csv_line_empty_fields() {
        assert_eq!(parse_csv_line("a,,c"), vec!["a", "", "c"]);
    }

    #[test]
    fn test_format_csv_roundtrip() {
        let rows = vec![
            vec!["Name".to_string(), "Score".to_string()],
            vec!["Alice".to_string(), "100".to_string()],
        ];
        let csv = format_csv(&rows);
        assert_eq!(csv, "Name,Score\nAlice,100");
    }

    #[test]
    fn test_format_csv_with_commas() {
        let rows = vec![vec!["hello, world".to_string()]];
        let csv = format_csv(&rows);
        assert_eq!(csv, "\"hello, world\"");
    }
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
