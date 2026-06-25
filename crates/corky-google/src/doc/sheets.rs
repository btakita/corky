use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use crate::filter::gmail_auth;

const DRIVE_FILES_API: &str = "https://www.googleapis.com/drive/v3/files";

const SHEETS_WRITE_API: &str = "https://sheets.googleapis.com/v4/spreadsheets";

const SHEETS_API: &str = "https://sheets.googleapis.com/v4/spreadsheets";

#[derive(Debug, Eq, PartialEq)]
struct CreatedSpreadsheet {
    id: String,
    url: String,
}

/// Extract a Google Sheets spreadsheet ID from a URL or raw ID.
pub fn parse_sheet_id(input: &str) -> &str {
    // Handle URLs like https://docs.google.com/spreadsheets/d/SHEET_ID/edit
    if let Some(rest) = input.strip_prefix("https://docs.google.com/spreadsheets/d/") {
        return rest.split('/').next().unwrap_or(rest);
    }
    input
}

/// Create a new Google spreadsheet and print its ID and URL.
pub fn create(title: &str, account: Option<&str>) -> Result<()> {
    if title.trim().is_empty() {
        bail!("Spreadsheet title cannot be empty");
    }

    let token = get_sheets_token(account)?;

    eprintln!("Creating spreadsheet {title}...");
    let created = create_spreadsheet(title, &token)?;

    println!("id: {}", created.id);
    println!("url: {}", created.url);

    Ok(())
}

/// Share a Google spreadsheet with a user email address.
pub fn share(
    sheet: &str,
    email: &str,
    role: &str,
    notify: bool,
    account: Option<&str>,
) -> Result<()> {
    let sheet_id = parse_sheet_id(sheet);
    let role = normalize_share_role(role)?;
    if email.trim().is_empty() {
        bail!("Share email cannot be empty");
    }

    let token = get_drive_file_token(account)?;

    eprintln!("Sharing spreadsheet {sheet_id} with {email} as {role}...");
    create_drive_permission(sheet_id, email, role, notify, &token)?;
    println!(
        "Shared {} with {email} as {role}.",
        spreadsheet_url(sheet_id)
    );

    Ok(())
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
///
/// #ckysheets: the clear and the write are two separate HTTP calls, so a failure
/// between them would leave the tab empty with no backup. To make that safe we
/// snapshot the existing rows **before** clearing. If snapshotting fails we abort
/// before touching the tab; if the value write fails we both save the snapshot to
/// a local `<file>.bak` CSV and attempt to restore it to the tab, so a partial
/// failure never silently discards the previous contents.
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

    // Snapshot before clearing. On snapshot failure, bail without clearing so the
    // tab is left untouched (no data loss).
    let backup = fetch_rows(sheet_id, Some(&clear_range), &token).context(
        "failed to snapshot existing tab before clearing; push aborted to avoid data loss",
    )?;

    eprintln!("Clearing tab {tab}...");
    clear_values(sheet_id, &clear_range, &token)?;

    let start_range = tab_start_range(tab);
    eprintln!("Writing {} rows to tab {tab}...", values.len());
    match update_values(sheet_id, &start_range, &values, &token) {
        Ok(updated) => {
            println!(
                "Synced {} rows to {tab} ({} cells updated).",
                values.len(),
                updated
            );
            Ok(())
        }
        Err(write_err) if backup.is_empty() => {
            // Tab was empty before the clear — nothing to restore.
            Err(write_err)
        }
        Err(write_err) => {
            eprintln!(
                "Write failed after clearing {tab}; restoring previous {} row(s)...",
                backup.len()
            );
            // Persist the snapshot locally first so it survives even if the
            // restore write also fails.
            let backup_path = backup_csv_path(file);
            if let Err(save_err) = std::fs::write(&backup_path, format_csv(&backup)) {
                eprintln!(
                    "  warning: could not write local backup {}: {save_err}",
                    backup_path.display()
                );
            }
            match update_values(sheet_id, &start_range, &backup, &token) {
                Ok(_) => bail!(
                    "Sheets push to {tab} failed; previous contents restored. \
                     Backup saved to {}. Cause: {write_err}",
                    backup_path.display()
                ),
                Err(restore_err) => bail!(
                    "Sheets push to {tab} failed AND rollback failed — the tab may be empty. \
                     Previous contents saved to {}; restore it with `corky sheets push`. \
                     Push error: {write_err}; rollback error: {restore_err}",
                    backup_path.display()
                ),
            }
        }
    }
}

/// Local rollback-backup path for a failed `sheets push`: the source CSV path
/// with a `.bak` suffix appended (`data.csv` → `data.csv.bak`).
fn backup_csv_path(file: &Path) -> PathBuf {
    let mut name = file
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    file.with_file_name(name)
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

fn get_drive_file_token(account: Option<&str>) -> Result<String> {
    gmail_auth::get_access_token_for_user(Some("default"), gmail_auth::DRIVE_FILE_SCOPE, account)
}

fn sheets_command_scope() -> &'static str {
    gmail_auth::SHEETS_SCOPE
}

fn create_spreadsheet(title: &str, token: &str) -> Result<CreatedSpreadsheet> {
    if title.trim().is_empty() {
        bail!("Spreadsheet title cannot be empty");
    }

    let url = format!("{SHEETS_API}?fields=spreadsheetId,spreadsheetUrl,properties.title");
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_json(create_spreadsheet_request(title));

    match resp {
        Ok(r) => {
            let result: serde_json::Value = r.into_json()?;
            let id = result["spreadsheetId"]
                .as_str()
                .ok_or_else(|| {
                    anyhow::anyhow!("Sheets API response did not include spreadsheetId")
                })?
                .to_string();
            let url = result["spreadsheetUrl"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| spreadsheet_url(&id));
            Ok(CreatedSpreadsheet { id, url })
        }
        Err(ureq::Error::Status(401, _)) => {
            bail!(
                "Sheets API: unauthorized (401). Re-run `corky auth --scope sheets` or `corky auth --scope workspace`."
            )
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Sheets API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}

fn create_spreadsheet_request(title: &str) -> serde_json::Value {
    serde_json::json!({
        "properties": {
            "title": title
        }
    })
}

fn create_drive_permission(
    sheet_id: &str,
    email: &str,
    role: &str,
    notify: bool,
    token: &str,
) -> Result<()> {
    if email.trim().is_empty() {
        bail!("Share email cannot be empty");
    }

    let url = share_permission_url(sheet_id, notify);
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_json(share_permission_request(email, role)?);

    match resp {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(401, _)) => {
            bail!(
                "Drive API: unauthorized (401). Re-run `corky auth --scope drive` or `corky auth --scope workspace`."
            )
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Drive permissions error (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("Drive permissions request failed: {}", e),
    }
}

fn share_permission_request(email: &str, role: &str) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "type": "user",
        "role": normalize_share_role(role)?,
        "emailAddress": email,
    }))
}

fn normalize_share_role(role: &str) -> Result<&'static str> {
    match role {
        "reader" => Ok("reader"),
        "writer" => Ok("writer"),
        "commenter" => Ok("commenter"),
        other => bail!("Unsupported share role: {other}. Use reader, writer, or commenter."),
    }
}

fn share_permission_url(sheet_id: &str, notify: bool) -> String {
    format!(
        "{}/{}/permissions?supportsAllDrives=true&sendNotificationEmail={}",
        DRIVE_FILES_API,
        urlencode(sheet_id),
        notify
    )
}

fn spreadsheet_url(sheet_id: &str) -> String {
    format!("https://docs.google.com/spreadsheets/d/{sheet_id}/edit")
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

fn urlencode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sheet_id_raw() {
        assert_eq!(parse_sheet_id("abc123"), "abc123");
    }

    #[test]
    fn test_backup_csv_path() {
        // #ckysheets: failed push writes a sibling `<file>.bak` backup.
        assert_eq!(
            backup_csv_path(Path::new("/tmp/data.csv")),
            PathBuf::from("/tmp/data.csv.bak")
        );
        assert_eq!(
            backup_csv_path(Path::new("export")),
            PathBuf::from("export.bak")
        );
        // Nested path keeps its directory.
        assert_eq!(
            backup_csv_path(Path::new("a/b/contacts.csv")),
            PathBuf::from("a/b/contacts.csv.bak")
        );
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
    fn test_create_spreadsheet_request() {
        assert_eq!(
            create_spreadsheet_request("MRH Leads"),
            serde_json::json!({
                "properties": {
                    "title": "MRH Leads"
                }
            })
        );
    }

    #[test]
    fn test_share_permission_request() {
        assert_eq!(
            share_permission_request("ron@example.com", "writer").unwrap(),
            serde_json::json!({
                "type": "user",
                "role": "writer",
                "emailAddress": "ron@example.com"
            })
        );
    }

    #[test]
    fn test_share_permission_request_rejects_invalid_role() {
        let err = share_permission_request("ron@example.com", "owner")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unsupported share role"));
    }

    #[test]
    fn test_share_permission_url() {
        assert_eq!(
            share_permission_url("sheet/id", false),
            "https://www.googleapis.com/drive/v3/files/sheet%2Fid/permissions?supportsAllDrives=true&sendNotificationEmail=false"
        );
        assert_eq!(
            share_permission_url("sheet-id", true),
            "https://www.googleapis.com/drive/v3/files/sheet-id/permissions?supportsAllDrives=true&sendNotificationEmail=true"
        );
    }

    #[test]
    fn test_spreadsheet_url() {
        assert_eq!(
            spreadsheet_url("abc123"),
            "https://docs.google.com/spreadsheets/d/abc123/edit"
        );
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
