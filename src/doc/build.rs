use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build a document from markdown.
///
/// Supports PDF (pandoc → HTML → weasyprint) and DOCX (pandoc native).
pub fn run(file: &Path, format: &str, template: Option<&str>, output: Option<&str>) -> Result<()> {
    if !file.exists() {
        bail!("Input file not found: {}", file.display());
    }

    let output_path = match output {
        Some(p) => PathBuf::from(p),
        None => file.with_extension(format),
    };

    let template_name = template
        .map(String::from)
        .or_else(|| read_frontmatter_template(file))
        .unwrap_or_else(|| "proposal".to_string());

    let css_path = crate::resolve::templates_dir()
        .join(format!("{template_name}.css"))
        .canonicalize()
        .unwrap_or_else(|_| crate::resolve::templates_dir().join(format!("{template_name}.css")));

    match format {
        "pdf" => build_pdf(file, &output_path, &css_path)?,
        "docx" => build_docx(file, &output_path, &css_path)?,
        _ => bail!("Unsupported format: {format}. Use 'pdf' or 'docx'."),
    }

    println!("Done: {}", output_path.display());
    Ok(())
}

fn build_pdf(input: &Path, output: &Path, css: &Path) -> Result<()> {
    check_tool("pandoc")?;
    check_tool("weasyprint")?;

    let tmp_html = std::env::temp_dir().join(format!("corky-doc-{}.html", std::process::id()));

    // pandoc markdown → HTML
    let mut pandoc_args = vec![
        input.as_os_str().to_owned(),
        "-o".into(),
        tmp_html.as_os_str().to_owned(),
        "--standalone".into(),
        "--metadata".into(),
        "title= ".into(),
    ];
    if css.exists() {
        pandoc_args.push("--css".into());
        pandoc_args.push(css.as_os_str().to_owned());
    }

    println!("Converting: {} → {}", input.display(), output.display());

    let status = Command::new("pandoc")
        .args(&pandoc_args)
        .status()
        .context("Failed to run pandoc")?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_html);
        bail!("pandoc failed (exit {})", status.code().unwrap_or(-1));
    }

    // weasyprint HTML → PDF
    let status = Command::new("weasyprint")
        .arg(&tmp_html)
        .arg(output)
        .status()
        .context("Failed to run weasyprint")?;

    let _ = std::fs::remove_file(&tmp_html);

    if !status.success() {
        bail!("weasyprint failed (exit {})", status.code().unwrap_or(-1));
    }

    Ok(())
}

fn build_docx(input: &Path, output: &Path, css: &Path) -> Result<()> {
    check_tool("pandoc")?;

    let mut pandoc_args = vec![
        input.as_os_str().to_owned(),
        "-o".into(),
        output.as_os_str().to_owned(),
    ];
    if css.exists() {
        pandoc_args.push("--css".into());
        pandoc_args.push(css.as_os_str().to_owned());
    }

    println!("Converting: {} → {}", input.display(), output.display());

    let status = Command::new("pandoc")
        .args(&pandoc_args)
        .status()
        .context("Failed to run pandoc")?;

    if !status.success() {
        bail!("pandoc failed (exit {})", status.code().unwrap_or(-1));
    }

    Ok(())
}

/// Check that a tool is available on PATH.
fn check_tool(name: &str) -> Result<()> {
    match Command::new("which").arg(name).output() {
        Ok(out) if out.status.success() => Ok(()),
        _ => bail!(
            "{name} not found. Install it:\n  \
             pandoc: https://pandoc.org/installing.html\n  \
             weasyprint: pip install weasyprint"
        ),
    }
}

/// Extract `template:` from YAML frontmatter.
fn read_frontmatter_template(file: &Path) -> Option<String> {
    let content = std::fs::read_to_string(file).ok()?;
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return None;
    }
    let after = &content[4..];
    let end = after.find("\n---")?;
    let yaml = &after[..end];
    for line in yaml.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("template:") {
            let val = val.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}
