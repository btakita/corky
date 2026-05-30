//! SMS Backup & Restore XML → corky conversations.
//!
//! Parses the XML format from the Android "SMS Backup & Restore" app
//! and converts each phone number thread into a corky conversation
//! using `merge_message_to_file()`.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use super::imap_sync::merge_message_to_file;
use super::types::Message;

// ---------------------------------------------------------------------------
// XML types (SMS Backup & Restore format)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct SmsBackup {
    #[serde(rename = "$value", default)]
    entries: Vec<SmsEntry>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum SmsEntry {
    Sms(SmsRecord),
    Mms(MmsRecord),
}

#[derive(Debug, serde::Deserialize)]
struct SmsRecord {
    #[serde(rename = "@address")]
    address: String,
    #[serde(rename = "@date")]
    date: String,
    #[serde(rename = "@type")]
    msg_type: String,
    #[serde(rename = "@body")]
    body: String,
    #[serde(rename = "@contact_name", default)]
    contact_name: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct MmsRecord {
    #[serde(rename = "@address", default)]
    address: Option<String>,
    #[serde(rename = "@date")]
    date: String,
    #[serde(rename = "@msg_box")]
    msg_box: Option<String>,
    #[serde(rename = "@contact_name", default)]
    contact_name: Option<String>,
    #[serde(default)]
    parts: Option<MmsParts>,
    #[serde(default)]
    addrs: Option<MmsAddrs>,
}

#[derive(Debug, serde::Deserialize)]
struct MmsParts {
    #[serde(rename = "part", default)]
    parts: Vec<MmsPart>,
}

#[derive(Debug, serde::Deserialize)]
struct MmsPart {
    #[serde(rename = "@ct")]
    ct: String,
    #[serde(rename = "@text", default)]
    text: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct MmsAddrs {
    #[serde(rename = "addr", default)]
    addrs: Vec<MmsAddr>,
}

#[derive(Debug, serde::Deserialize)]
struct MmsAddr {
    #[serde(rename = "@address")]
    address: String,
    #[serde(rename = "@type")]
    addr_type: String,
}

// ---------------------------------------------------------------------------
// Date conversion
// ---------------------------------------------------------------------------

/// Convert Java millisecond timestamp to RFC 2822 date string.
fn ms_to_rfc2822(ms_str: &str) -> String {
    ms_str
        .parse::<i64>()
        .ok()
        .and_then(|ms| {
            chrono::DateTime::from_timestamp(ms / 1000, ((ms % 1000) * 1_000_000) as u32)
        })
        .map(|dt| dt.format("%a, %d %b %Y %H:%M:%S +0000").to_string())
        .unwrap_or_else(|| ms_str.to_string())
}

// ---------------------------------------------------------------------------
// Phone number normalization
// ---------------------------------------------------------------------------

/// Normalize a phone number to a consistent key for thread grouping.
/// Strips everything except digits and leading +.
fn normalize_phone(raw: &str) -> String {
    let mut result = String::new();
    for (i, c) in raw.chars().enumerate() {
        if c.is_ascii_digit() || (i == 0 && c == '+') {
            result.push(c);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Import logic
// ---------------------------------------------------------------------------

/// Import SMS messages from an SMS Backup & Restore XML file.
pub fn run(path: &Path, label: &str, out_dir: &Path, account_name: &str) -> Result<()> {
    println!("SMS import: {}", path.display());

    let data = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let backup: SmsBackup = quick_xml::de::from_str(&data)
        .with_context(|| format!("Failed to parse XML from {}", path.display()))?;

    // Group messages by normalized phone number
    let mut threads: HashMap<String, Vec<Message>> = HashMap::new();
    let mut contact_names: HashMap<String, String> = HashMap::new();
    let mut msg_counter: u64 = 0;

    for entry in &backup.entries {
        match entry {
            SmsEntry::Sms(sms) => {
                let phone = normalize_phone(&sms.address);
                if phone.is_empty() {
                    continue;
                }

                let is_sent = sms.msg_type == "2";
                let from = if is_sent {
                    "Me".to_string()
                } else {
                    sms.contact_name
                        .clone()
                        .unwrap_or_else(|| sms.address.clone())
                };

                if let Some(name) = &sms.contact_name
                    && !name.is_empty()
                    && name != "(Unknown)"
                {
                    contact_names
                        .entry(phone.clone())
                        .or_insert_with(|| name.clone());
                }

                msg_counter += 1;
                let message = Message {
                    id: format!("sms:{}", msg_counter),
                    thread_id: format!("sms:{}", phone),
                    from,
                    to: if is_sent {
                        sms.address.clone()
                    } else {
                        String::new()
                    },
                    cc: String::new(),
                    date: ms_to_rfc2822(&sms.date),
                    subject: contact_names
                        .get(&phone)
                        .cloned()
                        .unwrap_or_else(|| sms.address.clone()),
                    body: sms.body.clone(),
                    message_id: None,
                };

                threads.entry(phone).or_default().push(message);
            }
            SmsEntry::Mms(mms) => {
                // Extract text parts from MMS
                let body = mms
                    .parts
                    .as_ref()
                    .map(|p| {
                        p.parts
                            .iter()
                            .filter(|part| part.ct == "text/plain")
                            .filter_map(|part| part.text.as_deref())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();

                if body.trim().is_empty() {
                    continue;
                }

                // Determine phone number from address or addrs
                let phone = mms
                    .address
                    .as_deref()
                    .map(normalize_phone)
                    .or_else(|| {
                        mms.addrs.as_ref().and_then(|addrs| {
                            addrs
                                .addrs
                                .iter()
                                .find(|a| a.addr_type == "137") // 137 = From
                                .map(|a| normalize_phone(&a.address))
                        })
                    })
                    .unwrap_or_default();

                if phone.is_empty() {
                    continue;
                }

                let is_sent = mms.msg_box.as_deref() == Some("2");
                let from = if is_sent {
                    "Me".to_string()
                } else {
                    mms.contact_name.clone().unwrap_or_else(|| phone.clone())
                };

                if let Some(name) = &mms.contact_name
                    && !name.is_empty()
                    && name != "(Unknown)"
                {
                    contact_names
                        .entry(phone.clone())
                        .or_insert_with(|| name.clone());
                }

                msg_counter += 1;
                let message = Message {
                    id: format!("mms:{}", msg_counter),
                    thread_id: format!("sms:{}", phone),
                    from,
                    to: if is_sent {
                        mms.address.clone().unwrap_or_default()
                    } else {
                        String::new()
                    },
                    cc: String::new(),
                    date: ms_to_rfc2822(&mms.date),
                    subject: contact_names
                        .get(&phone)
                        .cloned()
                        .unwrap_or_else(|| phone.clone()),
                    body,
                    message_id: None,
                };

                threads.entry(phone).or_default().push(message);
            }
        }
    }

    // Sort each thread by date and merge into conversation files
    let mut total = 0u32;
    for (phone, mut messages) in threads {
        messages.sort_by(|a, b| a.date.cmp(&b.date));

        let thread_id = format!("sms:{}", phone);
        let display_name = contact_names
            .get(&phone)
            .cloned()
            .unwrap_or_else(|| phone.clone());

        for msg in &messages {
            merge_message_to_file(out_dir, label, account_name, msg, &thread_id)?;
            total += 1;
        }

        println!(
            "  {} ({}) — {} message(s)",
            display_name,
            phone,
            messages.len()
        );
    }

    println!("SMS import complete: {} message(s) total.", total);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ms_to_rfc2822() {
        // 2024-10-09T19:42:23Z = 1728506543000 ms
        let rfc = ms_to_rfc2822("1728506543000");
        assert!(rfc.contains("2024"));
        assert!(rfc.contains("Oct"));
    }

    #[test]
    fn test_ms_to_rfc2822_invalid() {
        assert_eq!(ms_to_rfc2822("not-a-number"), "not-a-number");
    }

    #[test]
    fn test_normalize_phone() {
        assert_eq!(normalize_phone("+1 (555) 123-4567"), "+15551234567");
        assert_eq!(normalize_phone("5551234567"), "5551234567");
        assert_eq!(normalize_phone("+44 7911 123456"), "+447911123456");
    }

    #[test]
    fn test_parse_sms_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<smses count="2">
  <sms address="+15551234567" date="1728506543000" type="1"
       body="Hello!" contact_name="Alice" read="1" />
  <sms address="+15551234567" date="1728506600000" type="2"
       body="Hi Alice!" contact_name="Alice" read="1" />
</smses>"#;

        let backup: SmsBackup = quick_xml::de::from_str(xml).unwrap();
        assert_eq!(backup.entries.len(), 2);

        match &backup.entries[0] {
            SmsEntry::Sms(sms) => {
                assert_eq!(sms.address, "+15551234567");
                assert_eq!(sms.body, "Hello!");
                assert_eq!(sms.msg_type, "1"); // received
                assert_eq!(sms.contact_name.as_deref(), Some("Alice"));
            }
            _ => panic!("Expected SMS entry"),
        }

        match &backup.entries[1] {
            SmsEntry::Sms(sms) => {
                assert_eq!(sms.msg_type, "2"); // sent
                assert_eq!(sms.body, "Hi Alice!");
            }
            _ => panic!("Expected SMS entry"),
        }
    }

    #[test]
    fn test_import_sms_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path().join("conversations");
        std::fs::create_dir_all(&out_dir).unwrap();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<smses count="2">
  <sms address="+15551234567" date="1728506543000" type="1"
       body="Hello from Alice!" contact_name="Alice" read="1" />
  <sms address="+15551234567" date="1728506600000" type="2"
       body="Hi Alice, how are you?" contact_name="Alice" read="1" />
</smses>"#;

        let xml_path = dir.path().join("sms-backup.xml");
        std::fs::write(&xml_path, xml).unwrap();

        run(&xml_path, "sms", &out_dir, "sms-import").unwrap();

        let files: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
            .collect();
        assert_eq!(files.len(), 1);

        let content = std::fs::read_to_string(files[0].path()).unwrap();
        assert!(content.contains("Alice"));
        assert!(content.contains("Hello from Alice!"));
        assert!(content.contains("Hi Alice, how are you?"));
        assert!(content.contains("sms:"));
    }
}
