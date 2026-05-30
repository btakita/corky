use anyhow::{Context, Result};

use crate::config::corky_config::{self, GmailFilter};
use crate::resolve::data_dir;

/// Standalone filter config for parsing a separate TOML file (--input).
#[derive(serde::Deserialize)]
struct StandaloneFilterConfig {
    meta: Option<Meta>,
    filters: Vec<StandaloneFilter>,
}

#[derive(serde::Deserialize)]
struct Meta {
    name: Option<String>,
    email: Option<String>,
}

#[derive(serde::Deserialize)]
struct StandaloneFilter {
    label: Option<String>,
    #[serde(rename = "match", default)]
    match_fields: Vec<String>,
    addresses: Vec<String>,
    forward_to: Option<String>,
    #[serde(default)]
    star: bool,
    #[serde(default)]
    never_spam: bool,
    #[serde(default)]
    always_important: bool,
}

impl From<&StandaloneFilter> for GmailFilter {
    fn from(f: &StandaloneFilter) -> Self {
        GmailFilter {
            label: f.label.clone(),
            match_fields: f.match_fields.clone(),
            addresses: f.addresses.clone(),
            forward_to: f.forward_to.clone(),
            star: f.star,
            never_spam: f.never_spam,
            always_important: f.always_important,
        }
    }
}

pub fn run(input: Option<&str>, output: Option<&str>) -> Result<()> {
    if let Some(input_path) = input {
        // Standalone TOML file mode
        run_from_file(input_path, output)
    } else {
        // Read from .corky.toml [[gmail.filters]]
        run_from_config(output)
    }
}

/// Build from a standalone TOML file (backward compat, --input flag).
fn run_from_file(input: &str, output: Option<&str>) -> Result<()> {
    let input_path = std::path::PathBuf::from(input);
    if !input_path.exists() {
        anyhow::bail!("Input file not found: {}", input_path.display());
    }

    let output_path = match output {
        Some(p) => std::path::PathBuf::from(p),
        None => input_path.with_file_name("mailFilters.xml"),
    };

    let toml_str = std::fs::read_to_string(&input_path)
        .with_context(|| format!("Failed to read {}", input_path.display()))?;
    let config: StandaloneFilterConfig =
        toml::from_str(&toml_str).with_context(|| "Failed to parse filters TOML")?;

    let filters: Vec<GmailFilter> = config.filters.iter().map(GmailFilter::from).collect();
    let meta_name = config.meta.as_ref().and_then(|m| m.name.as_deref());
    let meta_email = config.meta.as_ref().and_then(|m| m.email.as_deref());

    let xml = build_xml(&filters, meta_name, meta_email);

    std::fs::write(&output_path, &xml)
        .with_context(|| format!("Failed to write {}", output_path.display()))?;

    println!(
        "Generated {} from {}",
        output_path.display(),
        input_path.display()
    );
    println!("  {} filter(s)", filters.len());
    Ok(())
}

/// Build from [[gmail.filters]] in .corky.toml.
fn run_from_config(output: Option<&str>) -> Result<()> {
    let config = corky_config::load_config(None)?;

    let gmail = config
        .gmail
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No [gmail] section in .corky.toml"))?;

    if gmail.filters.is_empty() {
        anyhow::bail!("No [[gmail.filters]] entries in .corky.toml");
    }

    let output_path = match output {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let dir = data_dir();
            dir.join("mailFilters.xml")
        }
    };

    // Resolve meta from owner + first Gmail account
    let meta_name = config
        .owner
        .as_ref()
        .map(|o| o.name.as_str())
        .filter(|n| !n.is_empty());
    let meta_email = config
        .accounts
        .values()
        .find(|a| a.provider == "gmail")
        .map(|a| a.user.as_str());

    let xml = build_xml(&gmail.filters, meta_name, meta_email);

    std::fs::write(&output_path, &xml)
        .with_context(|| format!("Failed to write {}", output_path.display()))?;

    println!("Generated {}", output_path.display());
    println!("  {} filter(s)", gmail.filters.len());
    Ok(())
}

fn build_xml(filters: &[GmailFilter], name: Option<&str>, email: Option<&str>) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");

    let mut xml = String::new();
    xml.push_str("<?xml version='1.0' encoding='UTF-8'?>\n");
    xml.push_str("<feed xmlns='http://www.w3.org/2005/Atom' xmlns:apps='http://schemas.google.com/apps/2006'>\n");
    xml.push_str("\t<title>Mail Filters</title>\n");
    xml.push_str(&format!("\t<updated>{now}</updated>\n"));

    if name.is_some() || email.is_some() {
        xml.push_str("\t<author>\n");
        if let Some(n) = name {
            xml.push_str(&format!("\t\t<name>{}</name>\n", escape_xml(n)));
        }
        if let Some(e) = email {
            xml.push_str(&format!("\t\t<email>{}</email>\n", escape_xml(e)));
        }
        xml.push_str("\t</author>\n");
    }

    for filt in filters {
        build_entry(&mut xml, filt);
    }

    xml.push_str("</feed>\n");
    xml
}

fn build_entry(xml: &mut String, filt: &GmailFilter) {
    xml.push_str("\t<entry>\n");
    xml.push_str("\t\t<category term='filter'></category>\n");
    xml.push_str("\t\t<title>Mail Filter</title>\n");
    xml.push_str("\t\t<content></content>\n");

    let addr_str = filt.addresses.join(" OR ");
    let match_fields = if filt.match_fields.is_empty() {
        vec!["from".to_string()]
    } else {
        filt.match_fields.clone()
    };

    // When multiple match fields, use hasTheWord with OR query to avoid
    // Gmail's AND behavior on separate from/to criteria.
    if match_fields.len() > 1 {
        let parts: Vec<String> = match_fields
            .iter()
            .map(|field| format!("{}:({})", field, addr_str))
            .collect();
        xml.push_str(&format!(
            "\t\t<apps:property name='hasTheWord' value='{}'/>\n",
            escape_xml(&parts.join(" OR "))
        ));
    } else {
        for field in &match_fields {
            xml.push_str(&format!(
                "\t\t<apps:property name='{}' value='{}'/>\n",
                escape_xml(field),
                escape_xml(&addr_str)
            ));
        }
    }

    if let Some(label) = &filt.label {
        xml.push_str(&format!(
            "\t\t<apps:property name='label' value='{}'/>\n",
            escape_xml(label)
        ));
    }

    if let Some(fwd) = &filt.forward_to {
        xml.push_str(&format!(
            "\t\t<apps:property name='forwardTo' value='{}'/>\n",
            escape_xml(fwd)
        ));
    }

    if filt.star {
        xml.push_str("\t\t<apps:property name='shouldStar' value='true'/>\n");
    }
    if filt.never_spam {
        xml.push_str("\t\t<apps:property name='shouldNeverSpam' value='true'/>\n");
    }
    if filt.always_important {
        xml.push_str("\t\t<apps:property name='shouldAlwaysMarkAsImportant' value='true'/>\n");
    }

    xml.push_str("\t\t<apps:property name='sizeOperator' value='s_sl'/>\n");
    xml.push_str("\t\t<apps:property name='sizeUnit' value='s_smb'/>\n");
    xml.push_str("\t</entry>\n");
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_xml_single_filter() {
        let filters = vec![GmailFilter {
            label: Some("test-label".to_string()),
            match_fields: vec!["from".to_string(), "to".to_string()],
            addresses: vec!["a@b.com".to_string(), "c@d.com".to_string()],
            forward_to: None,
            star: false,
            never_spam: false,
            always_important: true,
        }];
        let xml = build_xml(&filters, Some("Test"), Some("test@example.com"));
        assert!(xml.contains("<name>Test</name>"));
        assert!(xml.contains("<email>test@example.com</email>"));
        // Multiple match fields use hasTheWord with OR query instead of separate from/to
        assert!(xml.contains(
            "name='hasTheWord' value='from:(a@b.com OR c@d.com) OR to:(a@b.com OR c@d.com)'"
        ));
        assert!(!xml.contains("name='from'"));
        assert!(!xml.contains("name='to'"));
        assert!(xml.contains("name='label' value='test-label'"));
        assert!(xml.contains("shouldAlwaysMarkAsImportant"));
        assert!(!xml.contains("shouldStar"));
    }

    #[test]
    fn test_default_match_is_from() {
        let filters = vec![GmailFilter {
            label: Some("x".to_string()),
            match_fields: vec![],
            addresses: vec!["a@b.com".to_string()],
            forward_to: None,
            star: false,
            never_spam: false,
            always_important: false,
        }];
        let xml = build_xml(&filters, None, None);
        assert!(xml.contains("name='from' value='a@b.com'"));
        assert!(!xml.contains("name='to'"));
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("a&b"), "a&amp;b");
        assert_eq!(escape_xml("a<b>c"), "a&lt;b&gt;c");
    }
}
