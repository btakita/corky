use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::Path;

use crate::doc::{gdocs, sheets};
use crate::filter::gmail_auth;

const DRIVE_API: &str = "https://www.googleapis.com/drive/v3/files";

const GOOGLE_DOC_MIME: &str = "application/vnd.google-apps.document";
const GOOGLE_SHEET_MIME: &str = "application/vnd.google-apps.spreadsheet";
const GOOGLE_SLIDE_MIME: &str = "application/vnd.google-apps.presentation";
const GOOGLE_FORM_MIME: &str = "application/vnd.google-apps.form";
const GOOGLE_DRAWING_MIME: &str = "application/vnd.google-apps.drawing";

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const PPTX_MIME: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DriveHint {
    Document,
    Spreadsheet,
    Presentation,
    Drawing,
    Form,
    DriveFile,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DriveKind {
    GoogleDoc,
    GoogleSheet,
    GoogleSlide,
    GoogleDrawing,
    GoogleForm,
    Pdf,
    OfficeDocument,
    OfficeSpreadsheet,
    OfficePresentation,
    Text,
    Image,
    Other,
}

#[derive(Debug, Eq, PartialEq)]
struct DriveRef {
    id: String,
    hint: DriveHint,
}

#[derive(Debug)]
struct DriveFile {
    id: String,
    name: String,
    mime_type: String,
    web_view_link: Option<String>,
}

/// Print metadata for a Google Workspace/Drive file.
pub fn info(input: &str, account: Option<&str>) -> Result<()> {
    let token = drive_read_token(account)?;
    let file_ref = parse_drive_ref(input);
    let metadata = metadata(&file_ref.id, &token)?;

    println!("id: {}", metadata.id);
    println!("name: {}", metadata.name);
    println!("mime_type: {}", metadata.mime_type);
    println!("kind: {}", kind_label(kind_for_mime(&metadata.mime_type)));
    if let Some(link) = metadata.web_view_link {
        println!("web_view_link: {}", link);
    }

    Ok(())
}

/// Read a Workspace file when it has a text/table representation.
///
/// Existing exact Google Docs/Sheets URL paths keep using their narrow service
/// APIs so users do not need a broad Drive token for normal docs/sheets reads.
pub fn read(input: &str, format: &str, output: Option<&Path>, account: Option<&str>) -> Result<()> {
    let file_ref = parse_drive_ref(input);
    match file_ref.hint {
        DriveHint::Document => {
            ensure_doc_text_format(format)?;
            return gdocs::read(input, output, account);
        }
        DriveHint::Spreadsheet => {
            let normalized = normalized_format(format);
            let sheet_format = match normalized.as_str() {
                "auto" | "table" | "markdown" | "md" | "text" | "txt" => "table",
                "csv" => "csv",
                other => bail!("Unsupported Sheets read format: {other}. Use table or csv."),
            };
            return sheets::read(input, None, sheet_format, output, account);
        }
        _ => {}
    }

    let token = drive_read_token(account)?;
    let metadata = metadata(&file_ref.id, &token)?;
    match kind_for_mime(&metadata.mime_type) {
        DriveKind::GoogleDoc => {
            ensure_doc_text_format(format)?;
            gdocs::read(&metadata.id, output, account)
        }
        DriveKind::GoogleSheet => {
            let normalized = normalized_format(format);
            let sheet_format = match normalized.as_str() {
                "auto" | "table" | "markdown" | "md" | "text" | "txt" => "table",
                "csv" => "csv",
                other => bail!("Unsupported Sheets read format: {other}. Use table or csv."),
            };
            sheets::read(&metadata.id, None, sheet_format, output, account)
        }
        DriveKind::GoogleSlide => {
            let bytes = export_bytes(&metadata.id, "text/plain", &token)?;
            write_or_print_text(bytes, output)
        }
        DriveKind::GoogleForm => {
            bail!(
                "Google Forms metadata was detected, but Drive does not expose form responses/content through `corky doc read`. Export responses from the linked Sheet or use the Forms UI."
            )
        }
        DriveKind::GoogleDrawing => {
            bail!(
                "Google Drawings are image exports. Use `corky doc export {input} -o drawing.png`."
            )
        }
        DriveKind::Pdf
        | DriveKind::OfficeDocument
        | DriveKind::OfficeSpreadsheet
        | DriveKind::OfficePresentation
        | DriveKind::Image
        | DriveKind::Other => {
            bail!(
                "{} is a binary Drive file ({}). Use `corky doc export {input} -o FILE` to download it.",
                metadata.name,
                metadata.mime_type
            )
        }
        DriveKind::Text => {
            let bytes = download_bytes(&metadata.id, &token)?;
            write_or_print_text(bytes, output)
        }
    }
}

/// Export a Google Workspace file or download a binary Drive file.
pub fn export(input: &str, format: &str, output: &Path, account: Option<&str>) -> Result<()> {
    let token = drive_read_token(account)?;
    let file_ref = parse_drive_ref(input);
    let metadata = metadata(&file_ref.id, &token)?;
    let kind = kind_for_mime(&metadata.mime_type);
    let requested = export_format(format, output);

    let bytes = match kind {
        DriveKind::GoogleDoc
        | DriveKind::GoogleSheet
        | DriveKind::GoogleSlide
        | DriveKind::GoogleDrawing => {
            let mime = export_mime(kind, requested)?;
            export_bytes(&metadata.id, mime, &token)?
        }
        DriveKind::GoogleForm => {
            bail!(
                "Google Forms are not exportable through Drive files.export. Export the response Sheet instead."
            )
        }
        DriveKind::Pdf
        | DriveKind::OfficeDocument
        | DriveKind::OfficeSpreadsheet
        | DriveKind::OfficePresentation
        | DriveKind::Text
        | DriveKind::Image
        | DriveKind::Other => download_bytes(&metadata.id, &token)?,
    };

    std::fs::write(output, bytes)?;
    eprintln!(
        "Written {} ({}) to {}",
        metadata.name,
        metadata.mime_type,
        output.display()
    );
    Ok(())
}

/// Write to a Google Doc or replace the content of a binary Drive file.
pub fn write(input: &str, file: &Path, account: Option<&str>) -> Result<()> {
    let file_ref = parse_drive_ref(input);
    match file_ref.hint {
        DriveHint::Document => {
            return gdocs::write(input, file, account);
        }
        DriveHint::Spreadsheet => {
            bail!("Use `corky sheets write` or `corky sheets push` to update Google Sheets.")
        }
        _ => {}
    }

    let token = drive_read_token(account)?;
    let metadata = metadata(&file_ref.id, &token)?;
    match kind_for_mime(&metadata.mime_type) {
        DriveKind::GoogleDoc => gdocs::write(&metadata.id, file, account),
        DriveKind::GoogleSheet => {
            bail!("Use `corky sheets write` or `corky sheets push` to update Google Sheets.")
        }
        DriveKind::GoogleSlide => {
            bail!("Google Slides write support is not implemented. Export/upload a PPTX instead.")
        }
        DriveKind::GoogleDrawing => {
            bail!(
                "Google Drawings write support is not implemented. Export/upload an image instead."
            )
        }
        DriveKind::GoogleForm => {
            bail!("Google Forms cannot be replaced through Drive file media uploads.")
        }
        DriveKind::Pdf
        | DriveKind::OfficeDocument
        | DriveKind::OfficeSpreadsheet
        | DriveKind::OfficePresentation
        | DriveKind::Text
        | DriveKind::Image
        | DriveKind::Other => {
            let write_token = drive_file_token(account)?;
            update_binary(&metadata.id, file, &write_token)
        }
    }
}

pub fn local_mime_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) => match ext.as_str() {
            "pdf" => "application/pdf".to_string(),
            "doc" => "application/msword".to_string(),
            "docx" => DOCX_MIME.to_string(),
            "xls" => "application/vnd.ms-excel".to_string(),
            "xlsx" => XLSX_MIME.to_string(),
            "ppt" => "application/vnd.ms-powerpoint".to_string(),
            "pptx" => PPTX_MIME.to_string(),
            "odt" => "application/vnd.oasis.opendocument.text".to_string(),
            "ods" => "application/vnd.oasis.opendocument.spreadsheet".to_string(),
            "odp" => "application/vnd.oasis.opendocument.presentation".to_string(),
            "rtf" => "application/rtf".to_string(),
            "md" => "text/markdown".to_string(),
            "txt" => "text/plain".to_string(),
            "html" | "htm" => "text/html".to_string(),
            "csv" => "text/csv".to_string(),
            "tsv" => "text/tab-separated-values".to_string(),
            "json" => "application/json".to_string(),
            "png" => "image/png".to_string(),
            "jpg" | "jpeg" => "image/jpeg".to_string(),
            "gif" => "image/gif".to_string(),
            "webp" => "image/webp".to_string(),
            "svg" => "image/svg+xml".to_string(),
            _ => mime_guess::from_path(path)
                .first_raw()
                .unwrap_or("application/octet-stream")
                .to_string(),
        },
        None => "application/octet-stream".to_string(),
    }
}

pub fn google_workspace_mime_for_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) => match ext.as_str() {
            "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "html" | "htm" => Some(GOOGLE_DOC_MIME),
            "xls" | "xlsx" | "ods" | "csv" | "tsv" => Some(GOOGLE_SHEET_MIME),
            "ppt" | "pptx" | "odp" => Some(GOOGLE_SLIDE_MIME),
            _ => None,
        },
        None => None,
    }
}

fn drive_read_token(account: Option<&str>) -> Result<String> {
    gmail_auth::get_access_token_for_user(
        Some("default"),
        gmail_auth::DRIVE_READONLY_SCOPE,
        account,
    )
}

fn drive_file_token(account: Option<&str>) -> Result<String> {
    gmail_auth::get_access_token_for_user(Some("default"), gmail_auth::DRIVE_FILE_SCOPE, account)
}

fn metadata(file_id: &str, token: &str) -> Result<DriveFile> {
    let url = format!(
        "{}/{}?fields=id,name,mimeType,webViewLink&supportsAllDrives=true",
        DRIVE_API,
        urlencode(file_id)
    );
    let resp = api_get(token, &url, "Drive metadata")?;
    let value: serde_json::Value = resp.into_json()?;
    Ok(DriveFile {
        id: value["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Drive metadata response did not include id"))?
            .to_string(),
        name: value["name"].as_str().unwrap_or("untitled").to_string(),
        mime_type: value["mimeType"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Drive metadata response did not include mimeType"))?
            .to_string(),
        web_view_link: value["webViewLink"].as_str().map(str::to_string),
    })
}

fn export_bytes(file_id: &str, mime_type: &str, token: &str) -> Result<Vec<u8>> {
    let url = format!(
        "{}/{}/export?mimeType={}",
        DRIVE_API,
        urlencode(file_id),
        urlencode(mime_type)
    );
    api_get_bytes(token, &url, "Drive export")
}

fn download_bytes(file_id: &str, token: &str) -> Result<Vec<u8>> {
    let url = format!(
        "{}/{}?alt=media&supportsAllDrives=true",
        DRIVE_API,
        urlencode(file_id)
    );
    api_get_bytes(token, &url, "Drive download")
}

fn update_binary(file_id: &str, file: &Path, token: &str) -> Result<()> {
    let mime_type = local_mime_for_path(file);
    let content = std::fs::read(file)
        .with_context(|| format!("Failed to read replacement file {}", file.display()))?;
    let url = format!(
        "https://www.googleapis.com/upload/drive/v3/files/{}?uploadType=media&supportsAllDrives=true",
        urlencode(file_id)
    );

    let resp = ureq::patch(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &mime_type)
        .send_bytes(&content);

    match resp {
        Ok(_) => {
            eprintln!("Updated Drive file {} from {}", file_id, file.display());
            Ok(())
        }
        Err(ureq::Error::Status(401, _)) => {
            bail!("Drive API: unauthorized (401). Re-run `corky auth --scope drive`.")
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Drive update error (HTTP {}): {}", status, body);
        }
        Err(e) => bail!("Drive update request failed: {}", e),
    }
}

fn api_get(token: &str, url: &str, label: &str) -> Result<ureq::Response> {
    match ureq::get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
    {
        Ok(resp) => Ok(resp),
        Err(ureq::Error::Status(401, _)) => {
            bail!("{label}: unauthorized (401). Re-run `corky auth --scope drive-readonly`.")
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("{label} error (HTTP {status}): {body}");
        }
        Err(e) => bail!("{label} request failed: {e}"),
    }
}

fn api_get_bytes(token: &str, url: &str, label: &str) -> Result<Vec<u8>> {
    let mut reader = api_get(token, url, label)?.into_reader();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn write_or_print_text(bytes: Vec<u8>, output: Option<&Path>) -> Result<()> {
    let text = String::from_utf8(bytes).context("Drive export was not valid UTF-8 text")?;
    if let Some(path) = output {
        std::fs::write(path, text)?;
        eprintln!("Written to {}", path.display());
    } else {
        print!("{text}");
    }
    Ok(())
}

fn parse_drive_ref(input: &str) -> DriveRef {
    let url_specs = [
        ("https://docs.google.com/document/d/", DriveHint::Document),
        (
            "https://docs.google.com/spreadsheets/d/",
            DriveHint::Spreadsheet,
        ),
        (
            "https://docs.google.com/presentation/d/",
            DriveHint::Presentation,
        ),
        ("https://docs.google.com/drawings/d/", DriveHint::Drawing),
        ("https://docs.google.com/forms/d/", DriveHint::Form),
        ("https://drive.google.com/file/d/", DriveHint::DriveFile),
    ];

    for (prefix, hint) in url_specs {
        if let Some(rest) = input.strip_prefix(prefix) {
            return DriveRef {
                id: id_path_segment(rest, hint),
                hint,
            };
        }
    }

    if let Some(id) = query_param(input, "id") {
        return DriveRef {
            id,
            hint: DriveHint::DriveFile,
        };
    }

    DriveRef {
        id: input.to_string(),
        hint: DriveHint::Unknown,
    }
}

fn id_path_segment(rest: &str, hint: DriveHint) -> String {
    let mut segments = rest.split('/');
    let first = segments.next().unwrap_or(rest);
    if hint == DriveHint::Form && first == "e" {
        return segments.next().unwrap_or(first).to_string();
    }
    first.to_string()
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input.split_once('?')?.1;
    for part in query.split('&') {
        if let Some((key, value)) = part.split_once('=')
            && key == name
            && !value.is_empty()
        {
            return Some(value.to_string());
        }
    }
    None
}

fn kind_for_mime(mime_type: &str) -> DriveKind {
    match mime_type {
        GOOGLE_DOC_MIME => DriveKind::GoogleDoc,
        GOOGLE_SHEET_MIME => DriveKind::GoogleSheet,
        GOOGLE_SLIDE_MIME => DriveKind::GoogleSlide,
        GOOGLE_DRAWING_MIME => DriveKind::GoogleDrawing,
        GOOGLE_FORM_MIME => DriveKind::GoogleForm,
        "application/pdf" => DriveKind::Pdf,
        DOCX_MIME
        | "application/msword"
        | "application/vnd.oasis.opendocument.text"
        | "application/rtf" => DriveKind::OfficeDocument,
        XLSX_MIME
        | "application/vnd.ms-excel"
        | "application/vnd.oasis.opendocument.spreadsheet" => DriveKind::OfficeSpreadsheet,
        PPTX_MIME
        | "application/vnd.ms-powerpoint"
        | "application/vnd.oasis.opendocument.presentation" => DriveKind::OfficePresentation,
        "text/plain" | "text/markdown" | "text/csv" | "text/tab-separated-values" | "text/html" => {
            DriveKind::Text
        }
        mime if mime.starts_with("image/") => DriveKind::Image,
        _ => DriveKind::Other,
    }
}

fn kind_label(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::GoogleDoc => "google-doc",
        DriveKind::GoogleSheet => "google-sheet",
        DriveKind::GoogleSlide => "google-slide",
        DriveKind::GoogleDrawing => "google-drawing",
        DriveKind::GoogleForm => "google-form",
        DriveKind::Pdf => "pdf",
        DriveKind::OfficeDocument => "office-document",
        DriveKind::OfficeSpreadsheet => "office-spreadsheet",
        DriveKind::OfficePresentation => "office-presentation",
        DriveKind::Text => "text",
        DriveKind::Image => "image",
        DriveKind::Other => "other",
    }
}

fn ensure_doc_text_format(format: &str) -> Result<()> {
    let normalized = normalized_format(format);
    match normalized.as_str() {
        "auto" | "text" | "txt" | "markdown" | "md" => Ok(()),
        other => bail!("Unsupported Google Docs read format: {other}. Use text/markdown."),
    }
}

fn normalized_format(format: &str) -> String {
    format.trim().to_ascii_lowercase()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExportFormat {
    Auto,
    Text,
    Markdown,
    Csv,
    Pdf,
    Docx,
    Xlsx,
    Pptx,
    Png,
    Svg,
}

fn export_format(format: &str, output: &Path) -> ExportFormat {
    let normalized = normalized_format(format);
    match normalized.as_str() {
        "txt" | "text" => ExportFormat::Text,
        "md" | "markdown" => ExportFormat::Markdown,
        "csv" => ExportFormat::Csv,
        "pdf" => ExportFormat::Pdf,
        "docx" => ExportFormat::Docx,
        "xlsx" => ExportFormat::Xlsx,
        "pptx" => ExportFormat::Pptx,
        "png" => ExportFormat::Png,
        "svg" => ExportFormat::Svg,
        _ => match output
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref()
        {
            Some("txt") => ExportFormat::Text,
            Some("md") => ExportFormat::Markdown,
            Some("csv") => ExportFormat::Csv,
            Some("pdf") => ExportFormat::Pdf,
            Some("docx") => ExportFormat::Docx,
            Some("xlsx") => ExportFormat::Xlsx,
            Some("pptx") => ExportFormat::Pptx,
            Some("png") => ExportFormat::Png,
            Some("svg") => ExportFormat::Svg,
            _ => ExportFormat::Auto,
        },
    }
}

fn export_mime(kind: DriveKind, format: ExportFormat) -> Result<&'static str> {
    match (kind, format) {
        (DriveKind::GoogleDoc, ExportFormat::Auto | ExportFormat::Docx) => Ok(DOCX_MIME),
        (DriveKind::GoogleDoc, ExportFormat::Text) => Ok("text/plain"),
        (DriveKind::GoogleDoc, ExportFormat::Markdown) => Ok("text/markdown"),
        (DriveKind::GoogleDoc, ExportFormat::Pdf) => Ok("application/pdf"),
        (DriveKind::GoogleSheet, ExportFormat::Auto | ExportFormat::Xlsx) => Ok(XLSX_MIME),
        (DriveKind::GoogleSheet, ExportFormat::Csv) => Ok("text/csv"),
        (DriveKind::GoogleSheet, ExportFormat::Pdf) => Ok("application/pdf"),
        (DriveKind::GoogleSlide, ExportFormat::Auto | ExportFormat::Pptx) => Ok(PPTX_MIME),
        (DriveKind::GoogleSlide, ExportFormat::Text) => Ok("text/plain"),
        (DriveKind::GoogleSlide, ExportFormat::Pdf) => Ok("application/pdf"),
        (DriveKind::GoogleDrawing, ExportFormat::Auto | ExportFormat::Png) => Ok("image/png"),
        (DriveKind::GoogleDrawing, ExportFormat::Svg) => Ok("image/svg+xml"),
        (DriveKind::GoogleDrawing, ExportFormat::Pdf) => Ok("application/pdf"),
        (_, requested) => bail!(
            "Unsupported export format {:?} for {}",
            requested,
            kind_label(kind)
        ),
    }
}

fn urlencode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_drive_ref_extracts_google_doc_url() {
        assert_eq!(
            parse_drive_ref("https://docs.google.com/document/d/doc-id/edit"),
            DriveRef {
                id: "doc-id".to_string(),
                hint: DriveHint::Document
            }
        );
    }

    #[test]
    fn parse_drive_ref_extracts_sheet_url() {
        assert_eq!(
            parse_drive_ref("https://docs.google.com/spreadsheets/d/sheet-id/edit#gid=0"),
            DriveRef {
                id: "sheet-id".to_string(),
                hint: DriveHint::Spreadsheet
            }
        );
    }

    #[test]
    fn parse_drive_ref_extracts_form_e_url() {
        assert_eq!(
            parse_drive_ref("https://docs.google.com/forms/d/e/form-id/viewform"),
            DriveRef {
                id: "form-id".to_string(),
                hint: DriveHint::Form
            }
        );
    }

    #[test]
    fn parse_drive_ref_extracts_drive_file_url() {
        assert_eq!(
            parse_drive_ref("https://drive.google.com/file/d/file-id/view?usp=sharing"),
            DriveRef {
                id: "file-id".to_string(),
                hint: DriveHint::DriveFile
            }
        );
    }

    #[test]
    fn parse_drive_ref_extracts_open_id_query() {
        assert_eq!(
            parse_drive_ref("https://drive.google.com/open?id=file-id"),
            DriveRef {
                id: "file-id".to_string(),
                hint: DriveHint::DriveFile
            }
        );
    }

    #[test]
    fn classify_google_workspace_mime_types() {
        assert_eq!(kind_for_mime(GOOGLE_DOC_MIME), DriveKind::GoogleDoc);
        assert_eq!(kind_for_mime(GOOGLE_SHEET_MIME), DriveKind::GoogleSheet);
        assert_eq!(kind_for_mime(GOOGLE_SLIDE_MIME), DriveKind::GoogleSlide);
        assert_eq!(kind_for_mime(GOOGLE_FORM_MIME), DriveKind::GoogleForm);
        assert_eq!(kind_for_mime(GOOGLE_DRAWING_MIME), DriveKind::GoogleDrawing);
    }

    #[test]
    fn classify_office_and_binary_mime_types() {
        assert_eq!(kind_for_mime(DOCX_MIME), DriveKind::OfficeDocument);
        assert_eq!(kind_for_mime(XLSX_MIME), DriveKind::OfficeSpreadsheet);
        assert_eq!(kind_for_mime(PPTX_MIME), DriveKind::OfficePresentation);
        assert_eq!(kind_for_mime("application/pdf"), DriveKind::Pdf);
        assert_eq!(kind_for_mime("image/png"), DriveKind::Image);
    }

    #[test]
    fn export_defaults_follow_workspace_type() {
        assert_eq!(
            export_mime(DriveKind::GoogleDoc, ExportFormat::Auto).unwrap(),
            DOCX_MIME
        );
        assert_eq!(
            export_mime(DriveKind::GoogleSheet, ExportFormat::Auto).unwrap(),
            XLSX_MIME
        );
        assert_eq!(
            export_mime(DriveKind::GoogleSlide, ExportFormat::Auto).unwrap(),
            PPTX_MIME
        );
        assert_eq!(
            export_mime(DriveKind::GoogleDrawing, ExportFormat::Auto).unwrap(),
            "image/png"
        );
    }

    #[test]
    fn export_format_uses_output_extension_when_auto() {
        assert_eq!(
            export_format("auto", &PathBuf::from("deck.pdf")),
            ExportFormat::Pdf
        );
        assert_eq!(
            export_format("auto", &PathBuf::from("notes.txt")),
            ExportFormat::Text
        );
    }

    #[test]
    fn local_mime_for_path_supports_office_files() {
        assert_eq!(local_mime_for_path(Path::new("proposal.docx")), DOCX_MIME);
        assert_eq!(local_mime_for_path(Path::new("data.xlsx")), XLSX_MIME);
        assert_eq!(local_mime_for_path(Path::new("slides.pptx")), PPTX_MIME);
    }

    #[test]
    fn google_workspace_conversion_targets_supported_files() {
        assert_eq!(
            google_workspace_mime_for_path(Path::new("proposal.docx")),
            Some(GOOGLE_DOC_MIME)
        );
        assert_eq!(
            google_workspace_mime_for_path(Path::new("data.csv")),
            Some(GOOGLE_SHEET_MIME)
        );
        assert_eq!(
            google_workspace_mime_for_path(Path::new("slides.pptx")),
            Some(GOOGLE_SLIDE_MIME)
        );
        assert_eq!(google_workspace_mime_for_path(Path::new("scan.pdf")), None);
    }
}
