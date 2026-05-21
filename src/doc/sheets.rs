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
    let token = get_sheets_token(account)?;

    eprintln!("Fetching sheet data...");
    let rows = fetch_rows(sheet_id, range, &token)?;

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
    let token = get_sheets_token(account)?;

    let values = read_csv_values(file)?;

    if values.is_empty() {
        eprintln!("No data in CSV file.");
        return Ok(());
    }

    eprintln!("Writing {} rows to {}...", values.len(), range);
    let updated = update_values(sheet_id, range, &values, &token)?;
    println!("Updated {} cells.", updated);

    Ok(())
}

/// Pull a whole Google Sheet tab into a local CSV file.
pub fn pull_tab(sheet: &str, tab: &str, file: &Path, account: Option<&str>) -> Result<()> {
    let sheet_id = parse_sheet_id(sheet);
    let token = get_sheets_token(account)?;
    let range = tab_range(tab);

    eprintln!("Fetching tab {tab}...");
    let rows = fetch_rows(sheet_id, Some(&range), &token)?;
    let csv = format_csv(&rows);
    std::fs::write(file, csv)?;
    eprintln!(
        "Synced {} rows from {tab} to {}",
        rows.len(),
        file.display()
    );

    Ok(())
}

/// Push a local CSV file into a whole Google Sheet tab.
///
/// This is a tab-level sync, not a partial update: the tab is created if it is
/// missing, then cleared before values are written from A1.
pub fn push_tab(sheet: &str, tab: &str, file: &Path, account: Option<&str>) -> Result<()> {
    let sheet_id = parse_sheet_id(sheet);
    let token = get_sheets_token(account)?;
    let values = read_csv_values(file)?;

    if values.is_empty() {
        eprintln!("No data in CSV file.");
        return Ok(());
    }

    ensure_tab_exists(sheet_id, tab, &token)?;

    let clear_range = tab_range(tab);
    eprintln!("Clearing tab {tab}...");
    clear_values(sheet_id, &clear_range, &token)?;

    let start_range = tab_start_range(tab);
    eprintln!("Writing {} rows to tab {tab}...", values.len());
    let updated = update_values(sheet_id, &start_range, &values, &token)?;
    println!(
        "Synced {} rows to {tab} ({} cells updated).",
        values.len(),
        updated
    );

    Ok(())
}

/// Delete a Google Sheet tab by title.
pub fn delete_tab(sheet: &str, tab: &str, account: Option<&str>) -> Result<()> {
    let sheet_id = parse_sheet_id(sheet);
    let token = get_sheets_token(account)?;

    let tab_sheet_id = tab_sheet_id(sheet_id, tab, &token)?
        .ok_or_else(|| anyhow::anyhow!("Sheet tab '{tab}' was not found."))?;

    eprintln!("Deleting tab {tab}...");
    delete_sheet(sheet_id, tab_sheet_id, &token)?;
    println!("Deleted tab {tab}.");

    Ok(())
}

fn get_sheets_token(account: Option<&str>) -> Result<String> {
    gmail_auth::get_access_token_for_user(Some("default"), sheets_command_scope(), account)
}

fn sheets_command_scope() -> &'static str {
    gmail_auth::SHEETS_SCOPE
}

fn fetch_rows(sheet_id: &str, range: Option<&str>, token: &str) -> Result<Vec<Vec<String>>> {
    let selected_range = match range {
        Some(range) => range.to_string(),
        None => first_sheet_title(sheet_id, token)?,
    };
    let url = format!(
        "{}/{}/values/{}",
        SHEETS_API,
        sheet_id,
        encode_range(&selected_range)
    );
    let resp = api_get(token, &url)?;
    let data: serde_json::Value = resp.into_json()?;

    Ok(data["values"]
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
        .unwrap_or_default())
}

fn first_sheet_title(sheet_id: &str, token: &str) -> Result<String> {
    let meta_url = format!("{}/{}?fields=sheets.properties.title", SHEETS_API, sheet_id);
    let meta_resp = api_get(token, &meta_url)?;
    let meta: serde_json::Value = meta_resp.into_json()?;
    Ok(meta["sheets"][0]["properties"]["title"]
        .as_str()
        .unwrap_or("Sheet1")
        .to_string())
}

fn update_values(sheet_id: &str, range: &str, values: &[Vec<String>], token: &str) -> Result<u64> {
    let body = serde_json::json!({ "values": values });

    let url = format!(
        "{}/{}/values/{}?valueInputOption=USER_ENTERED",
        SHEETS_WRITE_API,
        sheet_id,
        encode_range(range)
    );

    let resp = ureq::put(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(&body);

    match resp {
        Ok(r) => {
            let result: serde_json::Value = r.into_json()?;
            Ok(result["updatedCells"].as_u64().unwrap_or(0))
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

fn clear_values(sheet_id: &str, range: &str, token: &str) -> Result<()> {
    let url = format!(
        "{}/{}/values/{}:clear",
        SHEETS_WRITE_API,
        sheet_id,
        encode_range(range)
    );

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(serde_json::json!({}));

    match resp {
        Ok(_) => Ok(()),
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

fn ensure_tab_exists(sheet_id: &str, tab: &str, token: &str) -> Result<()> {
    if tab_exists(sheet_id, tab, token)? {
        return Ok(());
    }

    eprintln!("Creating tab {tab}...");
    let url = format!("{}/{}:batchUpdate", SHEETS_WRITE_API, sheet_id);
    let body = serde_json::json!({
        "requests": [
            {
                "addSheet": {
                    "properties": {
                        "title": tab
                    }
                }
            }
        ]
    });

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(body);

    match resp {
        Ok(_) => Ok(()),
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

fn delete_sheet(sheet_id: &str, tab_sheet_id: i64, token: &str) -> Result<()> {
    let url = format!("{}/{}:batchUpdate", SHEETS_WRITE_API, sheet_id);
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(delete_sheet_request(tab_sheet_id));

    match resp {
        Ok(_) => Ok(()),
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

fn delete_sheet_request(tab_sheet_id: i64) -> serde_json::Value {
    serde_json::json!({
        "requests": [
            {
                "deleteSheet": {
                    "sheetId": tab_sheet_id
                }
            }
        ]
    })
}

fn tab_exists(sheet_id: &str, tab: &str, token: &str) -> Result<bool> {
    let meta_url = format!("{}/{}?fields=sheets.properties.title", SHEETS_API, sheet_id);
    let meta_resp = api_get(token, &meta_url)?;
    let meta: serde_json::Value = meta_resp.into_json()?;

    Ok(tab_title_exists(&meta, tab))
}

fn tab_sheet_id(sheet_id: &str, tab: &str, token: &str) -> Result<Option<i64>> {
    let meta_url = format!(
        "{}/{}?fields=sheets.properties(sheetId,title)",
        SHEETS_API, sheet_id
    );
    let meta_resp = api_get(token, &meta_url)?;
    let meta: serde_json::Value = meta_resp.into_json()?;

    Ok(tab_sheet_id_from_metadata(&meta, tab))
}

fn tab_title_exists(meta: &serde_json::Value, tab: &str) -> bool {
    meta["sheets"]
        .as_array()
        .map(|sheets| {
            sheets.iter().any(|sheet| {
                sheet["properties"]["title"]
                    .as_str()
                    .map(|title| title == tab)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn tab_sheet_id_from_metadata(meta: &serde_json::Value, tab: &str) -> Option<i64> {
    meta["sheets"].as_array()?.iter().find_map(|sheet| {
        let properties = &sheet["properties"];
        (properties["title"].as_str()? == tab).then(|| properties["sheetId"].as_i64())?
    })
}

fn read_csv_values(file: &Path) -> Result<Vec<Vec<String>>> {
    let csv_content = std::fs::read_to_string(file)?;
    Ok(parse_csv(&csv_content))
}

fn parse_csv(content: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = content.chars().peekable();

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
                row.push(std::mem::take(&mut current));
            }
            '\n' if !in_quotes => {
                row.push(std::mem::take(&mut current));
                rows.push(std::mem::take(&mut row));
            }
            '\r' if !in_quotes => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut current));
                rows.push(std::mem::take(&mut row));
            }
            other => current.push(other),
        }
    }

    if !current.is_empty() || !row.is_empty() {
        row.push(current);
        rows.push(row);
    }

    rows
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

fn tab_range(tab: &str) -> String {
    if needs_quoted_tab(tab) {
        format!("'{}'", tab.replace('\'', "''"))
    } else {
        tab.to_string()
    }
}

fn tab_start_range(tab: &str) -> String {
    format!("{}!A1", tab_range(tab))
}

fn needs_quoted_tab(tab: &str) -> bool {
    tab.chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
}

/// URL-encode a sheet range (e.g., "Sheet1!A1:D10" → "Sheet1%21A1%3AD10").
fn encode_range(range: &str) -> String {
    let mut encoded = String::new();
    for byte in range.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
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
    fn test_sheets_commands_request_read_write_scope() {
        assert_eq!(sheets_command_scope(), gmail_auth::SHEETS_SCOPE);
        assert_ne!(sheets_command_scope(), gmail_auth::SHEETS_READONLY_SCOPE);
    }

    #[test]
    fn test_encode_range() {
        assert_eq!(encode_range("Sheet1!A1:D10"), "Sheet1%21A1%3AD10");
        assert_eq!(encode_range("A1"), "A1");
        assert_eq!(
            encode_range("'Project Plan'!A1"),
            "%27Project%20Plan%27%21A1"
        );
    }

    #[test]
    fn test_parse_csv_line_simple() {
        assert_eq!(parse_csv("a,b,c"), vec![vec!["a", "b", "c"]]);
    }

    #[test]
    fn test_parse_csv_line_quoted() {
        assert_eq!(
            parse_csv(r#""hello, world",b"#),
            vec![vec!["hello, world", "b"]]
        );
    }

    #[test]
    fn test_parse_csv_line_escaped_quotes() {
        assert_eq!(
            parse_csv(r#""say ""hi""",b"#),
            vec![vec!["say \"hi\"", "b"]]
        );
    }

    #[test]
    fn test_parse_csv_line_empty_fields() {
        assert_eq!(parse_csv("a,,c"), vec![vec!["a", "", "c"]]);
    }

    #[test]
    fn test_parse_csv_multiline_quoted_field() {
        assert_eq!(
            parse_csv("name,notes\nAlice,\"line 1\nline 2\"\n"),
            vec![
                vec!["name".to_string(), "notes".to_string()],
                vec!["Alice".to_string(), "line 1\nline 2".to_string()],
            ]
        );
    }

    #[test]
    fn test_parse_csv_crlf() {
        assert_eq!(
            parse_csv("a,b\r\nc,d\r\n"),
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string(), "d".to_string()],
            ]
        );
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

    #[test]
    fn test_tab_range_quotes_names_with_spaces() {
        assert_eq!(tab_range("Sheet1"), "Sheet1");
        assert_eq!(tab_range("Project Plan"), "'Project Plan'");
        assert_eq!(tab_start_range("Project Plan"), "'Project Plan'!A1");
    }

    #[test]
    fn test_tab_range_escapes_single_quotes() {
        assert_eq!(tab_range("Bob's Plan"), "'Bob''s Plan'");
    }

    #[test]
    fn test_tab_title_exists_matches_metadata() {
        let meta = serde_json::json!({
            "sheets": [
                { "properties": { "title": "Sheet1" } },
                { "properties": { "title": "Temporary Test" } }
            ]
        });
        assert!(tab_title_exists(&meta, "Temporary Test"));
        assert!(!tab_title_exists(&meta, "Missing"));
    }

    #[test]
    fn test_tab_sheet_id_from_metadata() {
        let meta = serde_json::json!({
            "sheets": [
                { "properties": { "sheetId": 0, "title": "Sheet1" } },
                { "properties": { "sheetId": 42, "title": "Temporary Test" } }
            ]
        });
        assert_eq!(
            tab_sheet_id_from_metadata(&meta, "Temporary Test"),
            Some(42)
        );
        assert_eq!(tab_sheet_id_from_metadata(&meta, "Missing"), None);
    }

    #[test]
    fn test_delete_sheet_request() {
        assert_eq!(
            delete_sheet_request(42),
            serde_json::json!({
                "requests": [
                    {
                        "deleteSheet": {
                            "sheetId": 42
                        }
                    }
                ]
            })
        );
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
